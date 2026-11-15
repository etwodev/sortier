//! What a Sortier Remote server and its clients send each other, as JSON under `/api`. Every request but `hello`
//! and `login` carries `Authorization: Bearer <token>`.

use serde::{Deserialize, Serialize};
use sortier_library::library::{ItemId, Library};
use sortier_library::patch::Patch;

/// The port a server listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 7461;

/// `GET /api/hello`: what's at the address. Needs no login.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Hello {
    /// Always "Sortier Remote".
    pub server: String,
    pub version: String,
}

/// `POST /api/login`.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Login {
    pub password: String,
    /// What to call the device the token is for (its host name, say).
    pub device: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Token {
    pub token: String,
}

/// `GET /api/library`: the whole library and the version it's at.
#[derive(Serialize, Deserialize)]
pub struct LibraryState {
    pub version: u64,
    pub library: Library,
}

/// `GET /api/changes?since=<version>`: the patches since `since`, in order. Waits a while for one when there are
/// none yet. `reload` means too much changed to send patches, so fetch the whole library again.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Changes {
    pub version: u64,
    #[serde(default)]
    pub patches: Vec<Patch>,
    #[serde(default)]
    pub reload: bool,
}

/// `POST /api/patch`: a change made to the library at version `base`. Refused (`409 Conflict`) unless the library is
/// still at `base`.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PatchRequest {
    pub base: u64,
    pub patch: Patch,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Version {
    pub version: u64,
}

/// `POST /api/covers` (with the picture as the body) and `POST /api/covers/from-page`: where on the server the new
/// cover was saved, to put in an item's, sequence's or collection's `cover`.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CoverSaved {
    pub path: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CoverFromPage {
    pub item: ItemId,
    pub index: u32,
}

/// An error reply's body.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Problem {
    pub message: String,
}
