//! Talking to a Sortier Remote server: the requests and replies it understands ([`api`]), an HTTPS client that
//! trusts the server by its certificate's fingerprint, as SSH does ([`RemoteClient`]), a disk cache of what was
//! fetched with a size limit ([`Cache`]), and a file on the server read like a local one ([`RemoteFile`]), for
//! streaming video.

pub mod api;
mod cache;
mod client;
mod stream;

pub use cache::Cache;
pub use client::{Probe, RemoteClient, RemoteError, fingerprint_of};
pub use stream::RemoteFile;
