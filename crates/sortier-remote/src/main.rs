use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use sortier_remote::said;
use sortier_remote::setup::{Config, Devices, Identity};

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
  Windows   run sortier-remote-tray.exe (next to this program) and tick Start with Windows in its menu: it runs
            `sortier-remote serve` from the notification area whenever you're signed in, and restarts it if it
            crashes. To serve before anyone signs in, register a
            Task Scheduler task at startup running `sortier-remote serve --data <folder>` as SYSTEM, with no time
            limit (README.md has the PowerShell lines).
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
    Serve {
        /// Stops the server, cleanly, when its standard input closes: how the tray app stops the server it runs.
        #[arg(long, hide = true)]
        until_input_closes: bool,
    },
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
    let data = sortier_remote::open_data_folder(cli.data)?;
    let mut config = Config::load(&data)?;
    match cli.command {
        Command::Serve { until_input_closes } => {
            let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
            // Otherwise held here while the server runs, so it runs until it's ended.
            let _held = if until_input_closes {
                std::thread::spawn(move || {
                    let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
                    let _ = stop.send(());
                });
                None
            } else {
                Some(stop)
            };
            sortier_remote::serve(
                &data,
                |listening| {
                    if listening.folders == 0 {
                        eprintln!("{}", said::NO_FOLDERS);
                    }
                    println!("{}{}", said::LISTENING, listening.address);
                    println!("{}{}", said::FINGERPRINT, listening.fingerprint);
                },
                |message| eprintln!("{message}"),
                async move {
                    let _ = stopped.await;
                },
            )
        }
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
            if config.folders.contains(&path) {
                println!("{} is served already.", path.display());
                return Ok(());
            }
            config.folders.push(path.clone());
            config.save(&data)?;
            println!("Serving {}. A running server starts looking through it within a few seconds.", path.display());
            Ok(())
        }
        Command::RemoveFolder { path } => {
            let path = std::path::absolute(&path)?;
            if !config.folders.contains(&path) {
                bail!("{} isn't served", path.display());
            }
            config.folders.retain(|folder| *folder != path);
            config.save(&data)?;
            println!("No longer serving {}. Its files aren't touched.", path.display());
            Ok(())
        }
        Command::Folders => {
            for folder in &config.folders {
                println!("{}", folder.display());
            }
            Ok(())
        }
        Command::SetUploadFolder { path } => {
            let path = std::path::absolute(&path)?;
            let served = config.folders.iter().any(|folder| path.starts_with(folder));
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
