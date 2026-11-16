//! Sortier Remote: a headless server holding a Sortier library and its files. The `sortier-remote` command sets it up
//! and runs it in a terminal or as a service; on Windows, `sortier-remote-tray` runs it in the notification area.

mod server;
pub mod setup;
mod state;

use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use sortier_library::library::set_data_dir;

use crate::setup::{Config, Identity, data_folder};
use crate::state::Server;

/// How often a running server looks whether `config.toml` changed.
const CONFIG_CHECK: Duration = Duration::from_secs(5);

/// How long connections get to finish once the server is asked to stop.
const STOP_GRACE: Duration = Duration::from_secs(3);

/// How long after files are uploaded they're added to the library: they come several at a time.
const UPLOAD_SETTLE: Duration = Duration::from_secs(3);

/// How long after the library changes it's written to disk: it often changes many times in a row.
const SAVE_SETTLE: Duration = Duration::from_secs(2);

/// The server's data folder (the one given, else `SORTIER_REMOTE_DIR`, else the system's), made if it's missing and
/// set as the library's data folder.
pub fn open_data_folder(given: Option<PathBuf>) -> Result<PathBuf> {
    let data = data_folder(given);
    std::fs::create_dir_all(&data).with_context(|| format!("{} can't be made", data.display()))?;
    set_data_dir(data.clone());
    Ok(data)
}

/// What `sortier-remote serve` prints, one line each, which the tray app reads to follow the server it runs.
pub mod said {
    /// Followed by the address.
    pub const LISTENING: &str = "Sortier Remote is listening on ";
    /// Followed by the certificate's fingerprint.
    pub const FINGERPRINT: &str = "Certificate fingerprint: ";
    pub const NO_FOLDERS: &str = "No folders are served yet: add one with sortier-remote add-folder <path>.";
    /// Begins what's printed when the server stops with an error.
    pub const ERROR: &str = "Error: ";
}

/// A server that has started listening.
pub struct Listening {
    pub address: SocketAddr,
    pub fingerprint: String,
    pub folders: usize,
}

/// Runs the server kept in `data` until `stop` finishes or it fails. `started` is called once it's listening, and
/// `log` with anything worth knowing as it runs.
pub fn serve(
    data: &Path,
    started: impl FnOnce(&Listening),
    log: impl Fn(&str) + Send + Sync + 'static,
    stop: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let config = Config::load(data)?;
    if config.password.is_none() {
        bail!("set a password first: sortier-remote set-password");
    }
    let identity = Identity::load_or_make(data)?;
    let tls = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(
            vec![identity.certificate.clone().into()],
            rustls::pki_types::PrivateKeyDer::try_from(identity.key.clone()).map_err(anyhow::Error::msg)?,
        )?;
    let address: SocketAddr = format!("{}:{}", config.bind, config.port).parse().context("the bind address or port isn't valid")?;
    let listener = std::net::TcpListener::bind(address)
        .with_context(|| format!("{address} can't be listened on (is Sortier Remote already running, or another program using port {}?)", config.port))?;
    let rescan_every = Duration::from_secs(config.rescan_minutes.max(1) * 60);
    let server = Server::new(data.to_path_buf(), config, identity.fingerprint());
    started(&Listening {
        address,
        fingerprint: server.fingerprint.clone(),
        folders: server.config().folders.len(),
    });
    let log: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(log);
    let runtime = tokio::runtime::Runtime::new()?;
    let serving = server.clone();
    let saving_log = log.clone();
    let served = runtime.block_on(async move {
        let server = serving;
        let handle = axum_server::Handle::new();
        let stopping = handle.clone();
        tokio::spawn(async move {
            stop.await;
            stopping.graceful_shutdown(Some(STOP_GRACE));
        });
        tokio::spawn(keep_scanning(server.clone(), rescan_every, log.clone()));
        tokio::spawn(keep_saving(server.clone(), log));
        let config = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(tls));
        axum_server::from_tcp_rustls(listener, config)?
            .handle(handle)
            .serve(crate::server::router(server).into_make_service())
            .await
            .context("the server stopped")
    });
    runtime.shutdown_timeout(STOP_GRACE);
    if let Err(error) = server.save() {
        saving_log(&format!("The library couldn't be saved: {error:#}"));
    }
    served
}

/// Writes the library to disk a little after it changes, so a burst of changes is written once.
async fn keep_saving(server: Arc<Server>, log: Arc<dyn Fn(&str) + Send + Sync>) {
    loop {
        server.unsaved.notified().await;
        tokio::time::sleep(SAVE_SETTLE).await;
        let saving = server.clone();
        match tokio::task::spawn_blocking(move || saving.save()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => log(&format!("The library couldn't be saved: {error:#}")),
            Err(error) => log(&format!("The library couldn't be saved: {error}")),
        }
    }
}

/// Scans the folders at start, then every `every`, or when asked, or after `config.toml` changed (a folder added with
/// `sortier-remote add-folder`, say). Uploads are added a few seconds after they come, without a scan.
async fn keep_scanning(server: Arc<Server>, every: Duration, log: Arc<dyn Fn(&str) + Send + Sync>) {
    loop {
        let scanning = server.clone();
        if let Err(error) = tokio::task::spawn_blocking(move || scanning.scan()).await {
            log(&format!("A scan failed: {error}"));
        }
        let waiting = tokio::time::sleep(every);
        tokio::pin!(waiting);
        loop {
            tokio::select! {
                _ = &mut waiting => break,
                _ = server.rescan.notified() => break,
                _ = server.uploads_waiting.notified() => {
                    tokio::time::sleep(UPLOAD_SETTLE).await;
                    let adding = server.clone();
                    if let Err(error) = tokio::task::spawn_blocking(move || adding.add_uploads()).await {
                        log(&format!("Uploads couldn't be added: {error}"));
                    }
                }
                _ = tokio::time::sleep(CONFIG_CHECK) => {
                    if server.reload_config() {
                        break;
                    }
                }
            }
        }
    }
}
