use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::blocking::{Client, RequestBuilder, Response};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use sortier_library::library::ItemId;
use sortier_library::patch::Patch;

use crate::api::{Changes, CoverFromPage, CoverSaved, Hello, LibraryState, Login, PatchRequest, Problem, Token, Version};

/// How long an ordinary request may take.
const REQUEST_TIME: Duration = Duration::from_secs(30);
/// How long the server may hold a request for changes before answering that there are none.
const CHANGES_TIME: Duration = Duration::from_secs(60);

/// Why talking to a server failed.
#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("the server can't be reached: {0}")]
    Unreachable(String),
    /// The server's certificate isn't the one trusted: it was set up again, or something is in the way.
    #[error("the server's certificate isn't the one Sortier trusts; if it was set up again, connect to it again")]
    CertificateChanged,
    #[error("the password isn't right, or this device's login was taken back")]
    Unauthorized,
    /// The library changed on the server since the version a patch was made against.
    #[error("the library changed on the server at the same time")]
    Conflict,
    #[error("the server said: {0}")]
    Server(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// A certificate's fingerprint: the SHA-256 of its DER encoding, as lower-case hex.
pub fn fingerprint_of(der: &[u8]) -> String {
    Sha256::digest(der).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Accepts the server's certificate when its fingerprint is the trusted one (or any, when probing), and remembers
/// the fingerprint it saw. The handshake's signatures are still checked against the certificate.
#[derive(Debug)]
struct Pinned {
    trusted: Option<String>,
    seen: Mutex<Option<String>>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let seen = fingerprint_of(end_entity.as_ref());
        *self.seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(seen.clone());
        match &self.trusted {
            Some(trusted) if *trusted != seen => Err(rustls::Error::General("certificate fingerprint mismatch".into())),
            _ => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, signature, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, signature, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// An HTTPS client trusting only the certificate with fingerprint `trusted` (or any, with `None`), and the verifier
/// that records the fingerprint it met.
fn https_client(trusted: Option<&str>) -> Result<(Client, Arc<Pinned>), RemoteError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let pinned = Arc::new(Pinned {
        trusted: trusted.map(str::to_ascii_lowercase),
        seen: Mutex::new(None),
        provider: provider.clone(),
    });
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| RemoteError::Unreachable(error.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(pinned.clone())
        .with_no_client_auth();
    let client = Client::builder()
        .tls_backend_preconfigured(config)
        .connect_timeout(Duration::from_secs(10))
        .timeout(None)
        .build()
        .map_err(|error| RemoteError::Unreachable(error.to_string()))?;
    Ok((client, pinned))
}

/// What's at an address, and the fingerprint of the certificate it showed: for the user to compare with what
/// `sortier-remote fingerprint` prints on the server before trusting it.
#[derive(Clone, Debug)]
pub struct Probe {
    pub hello: Hello,
    pub fingerprint: String,
}

/// A logged-in connection to a Sortier Remote server. Every call blocks, so make them off the UI thread.
pub struct RemoteClient {
    base: String,
    token: String,
    http: Client,
    pinned: Arc<Pinned>,
}

/// `address` as the start of the server's API URLs, adding the default port when it names none.
fn api_base(address: &str) -> String {
    let address = address.trim().trim_start_matches("https://").trim_end_matches('/');
    let has_port = match address.rsplit_once(':') {
        Some((host, port)) => port.parse::<u16>().is_ok() && (!host.contains(':') || host.ends_with(']')),
        None => false,
    };
    if has_port {
        format!("https://{address}/api")
    } else {
        format!("https://{address}:{}/api", crate::api::DEFAULT_PORT)
    }
}

/// The reply's body as JSON, or the error it stands for.
fn json<T: DeserializeOwned>(response: Response) -> Result<T, RemoteError> {
    checked(response)?.json().map_err(|error| RemoteError::Server(format!("an unexpected reply: {error}")))
}

/// The reply, unless it's an error.
fn checked(response: Response) -> Result<Response, RemoteError> {
    match response.status() {
        status if status.is_success() => Ok(response),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(RemoteError::Unauthorized),
        StatusCode::CONFLICT => Err(RemoteError::Conflict),
        status => {
            let message = response.json::<Problem>().map(|problem| problem.message).unwrap_or_else(|_| status.to_string());
            Err(RemoteError::Server(message))
        }
    }
}

fn bytes(response: Response) -> Result<Vec<u8>, RemoteError> {
    let mut body = Vec::new();
    checked(response)?.read_to_end(&mut body)?;
    Ok(body)
}

impl RemoteClient {
    /// Connects to `address` (`host` or `host:port`) trusting whatever certificate it shows, to learn what it is
    /// and the certificate's fingerprint.
    pub fn probe(address: &str) -> Result<Probe, RemoteError> {
        let (http, pinned) = https_client(None)?;
        let base = api_base(address);
        let response = http
            .get(format!("{base}/hello"))
            .timeout(REQUEST_TIME)
            .send()
            .map_err(|error| RemoteError::Unreachable(error.to_string()))?;
        let hello: Hello = json(response)?;
        if hello.server != "Sortier Remote" {
            return Err(RemoteError::Server(format!("that's {}, not Sortier Remote", hello.server)));
        }
        let fingerprint = pinned.seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone().unwrap_or_default();
        Ok(Probe { hello, fingerprint })
    }

    /// Logs in to the server at `address` (trusting the certificate `fingerprint`) as device `device`, returning
    /// the token to connect with from now on.
    pub fn login(address: &str, fingerprint: &str, password: &str, device: &str) -> Result<String, RemoteError> {
        let client = Self::new(address, fingerprint, "")?;
        let login = Login {
            password: password.to_string(),
            device: device.to_string(),
        };
        let token: Token = client.call(client.http.post(client.url("login")).json(&login))?;
        Ok(token.token)
    }

    /// A connection to the server at `address`, trusting the certificate `fingerprint`, logged in with `token`.
    /// Nothing is sent until the first call.
    pub fn new(address: &str, fingerprint: &str, token: &str) -> Result<Self, RemoteError> {
        let (http, pinned) = https_client(Some(fingerprint))?;
        Ok(Self {
            base: api_base(address),
            token: token.to_string(),
            http,
            pinned,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{path}", self.base)
    }

    /// Sends `request` with the token, telling a changed certificate apart from other failures to connect.
    fn send(&self, request: RequestBuilder) -> Result<Response, RemoteError> {
        request.bearer_auth(&self.token).send().map_err(|error| {
            let seen = self.pinned.seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
            match (seen, &self.pinned.trusted) {
                (Some(seen), Some(trusted)) if seen != *trusted => RemoteError::CertificateChanged,
                _ => RemoteError::Unreachable(error.to_string()),
            }
        })
    }

    fn call<T: DeserializeOwned>(&self, request: RequestBuilder) -> Result<T, RemoteError> {
        json(self.send(request.timeout(REQUEST_TIME))?)
    }

    fn post_json<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T, RemoteError> {
        self.call(self.http.post(self.url(path)).json(body))
    }

    fn get_bytes(&self, path: &str) -> Result<Vec<u8>, RemoteError> {
        bytes(self.send(self.http.get(self.url(path)))?)
    }

    /// The whole library and its version.
    pub fn library(&self) -> Result<LibraryState, RemoteError> {
        self.call(self.http.get(self.url("library")))
    }

    /// The changes since version `since`, waiting up to about half a minute for one.
    pub fn changes(&self, since: u64) -> Result<Changes, RemoteError> {
        json(self.send(self.http.get(self.url("changes")).query(&[("since", since)]).timeout(CHANGES_TIME))?)
    }

    /// Sends a change made to the library at version `base`, returning the version it's at with it.
    /// [`RemoteError::Conflict`] when the library had moved on.
    pub fn send_patch(&self, base: u64, patch: &Patch) -> Result<u64, RemoteError> {
        let request = PatchRequest {
            base,
            patch: patch.clone(),
        };
        let version: Version = self.post_json("patch", &request)?;
        Ok(version.version)
    }

    /// Item `item`'s thumbnail, as a JPEG.
    pub fn thumbnail(&self, item: ItemId) -> Result<Vec<u8>, RemoteError> {
        self.get_bytes(&format!("thumb/{item}"))
    }

    /// Item `item`'s whole file.
    pub fn file(&self, item: ItemId) -> Result<Vec<u8>, RemoteError> {
        self.get_bytes(&format!("file/{item}"))
    }

    /// `length` bytes of item `item`'s file from `start` (fewer at its end), and the file's whole length.
    pub fn file_range(&self, item: ItemId, start: u64, length: u64) -> Result<(Vec<u8>, u64), RemoteError> {
        let range = format!("bytes={start}-{}", start + length.max(1) - 1);
        let response = self.send(self.http.get(self.url(&format!("file/{item}"))).header("Range", range))?;
        let total = response
            .headers()
            .get("Content-Range")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.rsplit('/').next())
            .and_then(|total| total.parse().ok())
            .or(response.content_length());
        let body = bytes(response)?;
        let total = total.unwrap_or(start + body.len() as u64);
        Ok((body, total))
    }

    /// Page `index` of document `item`, drawn on the server, as a JPEG.
    pub fn page(&self, item: ItemId, index: u32) -> Result<Vec<u8>, RemoteError> {
        self.get_bytes(&format!("page/{item}/{index}"))
    }

    /// The cover saved on the server at `path` (as an item's `cover` names it).
    pub fn cover(&self, path: &Path) -> Result<Vec<u8>, RemoteError> {
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        self.get_bytes(&format!("cover/{name}"))
    }

    /// Saves `picture` (a JPEG or PNG) on the server as a cover, returning where.
    pub fn upload_cover(&self, picture: Vec<u8>) -> Result<PathBuf, RemoteError> {
        let saved: CoverSaved = self.call(self.http.post(self.url("covers")).body(picture))?;
        Ok(PathBuf::from(saved.path))
    }

    /// Has the server make a cover from page `index` of item `item`, returning where it saved it.
    pub fn cover_from_page(&self, item: ItemId, index: u32) -> Result<PathBuf, RemoteError> {
        let saved: CoverSaved = self.post_json("covers/from-page", &CoverFromPage { item, index })?;
        Ok(PathBuf::from(saved.path))
    }

    /// Uploads the file at `local` to `relative` (a path with `/` between folders) inside the server's upload
    /// folder, calling `progress` with the bytes sent so far. The server adds it to the library shortly after.
    pub fn upload(&self, relative: &str, local: &Path, progress: impl Fn(u64) + Send + 'static) -> Result<(), RemoteError> {
        let file = std::fs::File::open(local)?;
        let length = file.metadata()?.len();
        let body = reqwest::blocking::Body::sized(Counting { inner: file, sent: 0, progress }, length);
        let response = self.send(self.http.post(self.url("upload")).query(&[("path", relative)]).body(body))?;
        checked(response).map(|_| ())
    }

    /// Asks the server to look through its folders again now.
    pub fn rescan(&self) -> Result<(), RemoteError> {
        checked(self.send(self.http.post(self.url("rescan")).timeout(REQUEST_TIME))?).map(|_| ())
    }
}

/// A reader that reports how much has been read.
struct Counting<R, F> {
    inner: R,
    sent: u64,
    progress: F,
}

impl<R: Read, F: Fn(u64)> Read for Counting<R, F> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.sent += read as u64;
        (self.progress)(self.sent);
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_get_the_default_port() {
        assert_eq!(api_base("nas.local"), "https://nas.local:7461/api");
        assert_eq!(api_base("https://nas.local:9000/"), "https://nas.local:9000/api");
        assert_eq!(api_base("192.168.1.5:8443"), "https://192.168.1.5:8443/api");
        assert_eq!(api_base("[::1]:8443"), "https://[::1]:8443/api");
        assert_eq!(fingerprint_of(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
