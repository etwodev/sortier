mod server;
mod setup;
mod state;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand};
use sortier_library::library::{Library, set_data_dir};

use crate::setup::{Config, Devices, Identity, data_folder};
use crate::state::Server;

const ABOUT: &str = "Sortier Remote holds a Sortier library and its files, so the Sortier app can browse, read, play, \
tag and upload to it over the network. It scans its folders and makes thumbnails itself, and keeps everyone's \
edits in step.

Getting started:
  sortier-remote set-password
  sortier-remote add-folder /path/to/your/pictures
  sortier-remote serve

Then in Sortier choose Connect to Sortier Remote from the vault menu, and check the fingerprint it shows against \
`sortier-remote fingerprint`.

Running it all the time:
  Windows   sc.exe create SortierRemote binPath= \"C:\\path\\sortier-remote.exe serve --data C:\\SortierRemote\" start= auto
  Linux     a systemd unit with ExecStart=/usr/local/bin/sortier-remote serve, then systemctl enable --now
  macOS     a launchd agent in ~/Library/LaunchAgents running sortier-remote serve, with RunAtLoad and KeepAlive";

#[derive(Parser)]
#[command(name = "sortier-remote", version, about = "Sortier Remote: a Sortier library served over the network", long_about = ABOUT)]
struct Cli {
    /// The folder the server keeps its settings, library, thumbnails and certificate in (or set SORTIER_REMOTE_DIR).
    #[arg(long, global = true)]
    data: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Runs the server.
    Serve,
    /// Sets the password devices log in with: asked for on the terminal, or taken from SORTIER_REMOTE_PASSWORD.
    SetPassword,
    /// Serves the pictures, videos and documents in a folder (and its subfolders).
    AddFolder { path: PathBuf },
    /// Stops serving a folder. Its files aren't touched.
    RemoveFolder { path: PathBuf },
    /// Lists the folders served.
    Folders,
    /// Sets the folder uploads go into. It should be inside a served folder, so uploads appear in the library.
    SetUploadFolder { path: PathBuf },
    /// Prints the certificate's fingerprint, to check against what Sortier shows when connecting.
    Fingerprint,
    /// Lists the devices logged in.
    Devices,
    /// Logs every device out; each must log in with the password again.
    ForgetDevices,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let data = data_folder(cli.data);
    std::fs::create_dir_all(&data).with_context(|| format!("{} can't be made", data.display()))?;
    set_data_dir(data.clone());
    let mut config = Config::load(&data)?;
    match cli.command {
        Command::Serve => serve(data, config),
        Command::SetPassword => {
            let password = match std::env::var("SORTIER_REMOTE_PASSWORD") {
                Ok(password) => password,
                Err(_) => {
                    let password = rpassword::prompt_password("New password: ")?;
                    if rpassword::prompt_password("Again: ")? != password {
                        bail!("the passwords don't match");
                    }
                    password
                }
            };
            config.set_password(&password)?;
            config.save(&data)?;
            println!("The password is set.");
            Ok(())
        }
        Command::AddFolder { path } => {
            let path = std::path::absolute(&path)?;
            if !path.is_dir() {
                bail!("{} isn't a folder", path.display());
            }
            let mut library = Library::load(data.join("library.json"));
            if library.add_root(path.clone()) {
                library.save();
                println!("Serving {}. A running server picks it up at its next scan.", path.display());
            } else {
                println!("{} is served already.", path.display());
            }
            Ok(())
        }
        Command::RemoveFolder { path } => {
            let path = std::path::absolute(&path)?;
            let mut library = Library::load(data.join("library.json"));
            library.remove_root(&path);
            library.save();
            println!("No longer serving {}.", path.display());
            Ok(())
        }
        Command::Folders => {
            for root in &Library::load(data.join("library.json")).roots {
                println!("{}", root.display());
            }
            Ok(())
        }
        Command::SetUploadFolder { path } => {
            let path = std::path::absolute(&path)?;
            let served = Library::load(data.join("library.json")).roots.iter().any(|root| path.starts_with(root));
            if !served {
                eprintln!("Note: {} isn't inside a served folder, so uploads won't appear in the library.", path.display());
            }
            config.upload_folder = Some(path);
            config.save(&data)
        }
        Command::Fingerprint => {
            println!("{}", Identity::load_or_make(&data)?.fingerprint());
            Ok(())
        }
        Command::Devices => {
            for device in Devices::load(&data).tokens.values() {
                println!("{device}");
            }
            Ok(())
        }
        Command::ForgetDevices => {
            Devices::default().save(&data)?;
            println!("Every device is logged out. Restart a running server for it to notice.");
            Ok(())
        }
    }
}

fn serve(data: PathBuf, config: Config) -> Result<()> {
    if config.password.is_none() {
        bail!("set a password first: sortier-remote set-password");
    }
    let identity = Identity::load_or_make(&data)?;
    let tls = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(
            vec![identity.certificate.clone().into()],
            rustls::pki_types::PrivateKeyDer::try_from(identity.key.clone()).map_err(anyhow::Error::msg)?,
        )?;
    let address: SocketAddr = format!("{}:{}", config.bind, config.port).parse().context("the bind address or port isn't valid")?;
    let rescan_every = Duration::from_secs(config.rescan_minutes.max(1) * 60);
    let server = Server::new(data, config, identity.fingerprint());
    if server.held().library.roots.is_empty() {
        eprintln!("No folders are served yet: add one with sortier-remote add-folder <path>.");
    }
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        tokio::spawn(keep_scanning(server.clone(), rescan_every));
        println!("Sortier Remote is listening on {address}");
        println!("Certificate fingerprint: {}", server.fingerprint);
        let config = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(tls));
        axum_server::bind_rustls(address, config)
            .serve(crate::server::router(server).into_make_service())
            .await
            .context("the server stopped")
    })
}

/// Scans the folders at start, then every `every`, or soon after being asked (by an upload, say).
async fn keep_scanning(server: Arc<Server>, every: Duration) {
    loop {
        let scanning = server.clone();
        if let Err(error) = tokio::task::spawn_blocking(move || scanning.scan()).await {
            eprintln!("A scan failed: {error}");
        }
        tokio::select! {
            _ = tokio::time::sleep(every) => {}
            _ = server.rescan.notified() => {
                // Uploads come several files at a time; one scan after the last will do.
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    }
}
