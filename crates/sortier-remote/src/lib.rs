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

/// The server's data folder (the one given, else `SORTIER_REMOTE_DIR`, else the system's), made if it's missing and
/// set as the library's data folder.
pub fn open_data_folder(given: Option<PathBuf>) -> Result<PathBuf> {
    let data = data_folder(given);
    std::fs::create_dir_all(&data).with_context(|| format!("{} can't be made", data.display()))?;
    set_data_dir(data.clone());
    Ok(data)
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
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let handle = axum_server::Handle::new();
        let stopping = handle.clone();
        tokio::spawn(async move {
            stop.await;
            stopping.graceful_shutdown(Some(STOP_GRACE));
        });
        tokio::spawn(keep_scanning(server.clone(), rescan_every, Arc::new(log)));
        let config = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(tls));
        axum_server::from_tcp_rustls(listener, config)?
            .handle(handle)
            .serve(crate::server::router(server).into_make_service())
            .await
            .context("the server stopped")
    })
}

/// Scans the folders at start, then every `every`, or soon after being asked (by an upload, say), or after
/// `config.toml` changed (a folder added with `sortier-remote add-folder`, say).
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
                _ = server.rescan.notified() => {
                    // Uploads come several files at a time; one scan after the last will do.
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    break;
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
