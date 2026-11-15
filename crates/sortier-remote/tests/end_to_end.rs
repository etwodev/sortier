//! Runs the real `sortier-remote` program on a folder of test files and uses it the way the app does, through
//! `sortier-client`.

use std::path::Path;
use std::process::{Child, Command};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sortier_client::{RemoteClient, RemoteError, RemoteFile};
use sortier_library::library::MediaKind;

const PASSWORD: &str = "a long test password";

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn remote(data: &Path, arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_sortier-remote"))
        .arg("--data")
        .arg(data)
        .args(arguments)
        .env("SORTIER_REMOTE_PASSWORD", PASSWORD)
        .output()
        .unwrap()
}

fn png(colour: [u8; 3]) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(64, 48, image::Rgba([colour[0], colour[1], colour[2], 255]));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(image).write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
    bytes
}

fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(found) = check() {
            return found;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn the_app_can_browse_edit_stream_and_upload() {
    let data = tempfile::tempdir().unwrap();
    let pictures = tempfile::tempdir().unwrap();
    std::fs::write(pictures.path().join("red.png"), png([220, 30, 30])).unwrap();
    std::fs::write(pictures.path().join("blue.png"), png([30, 30, 220])).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    std::fs::write(data.path().join("config.toml"), format!("port = {port}\nbind = \"127.0.0.1\"\n")).unwrap();
    assert!(remote(data.path(), &["set-password"]).status.success());
    assert!(remote(data.path(), &["add-folder", pictures.path().to_str().unwrap()]).status.success());
    let fingerprint = String::from_utf8(remote(data.path(), &["fingerprint"]).stdout).unwrap().trim().to_string();
    let _server = Running(
        Command::new(env!("CARGO_BIN_EXE_sortier-remote"))
            .arg("--data")
            .arg(data.path())
            .arg("serve")
            .spawn()
            .unwrap(),
    );
    let address = format!("127.0.0.1:{port}");

    let probe = wait_for("the server", || RemoteClient::probe(&address).ok());
    assert_eq!(probe.fingerprint, fingerprint, "the certificate is the one the CLI prints");
    assert!(matches!(RemoteClient::login(&address, &fingerprint, "wrong", "test"), Err(RemoteError::Unauthorized)));
    assert!(matches!(
        RemoteClient::login(&address, &"0".repeat(64), PASSWORD, "test"),
        Err(RemoteError::CertificateChanged)
    ));
    let token = RemoteClient::login(&address, &fingerprint, PASSWORD, "test").unwrap();
    let client = Arc::new(RemoteClient::new(&address, &fingerprint, &token).unwrap());
    assert!(matches!(RemoteClient::new(&address, &fingerprint, "bad").unwrap().library(), Err(RemoteError::Unauthorized)));

    let state = wait_for("the scan", || client.library().ok().filter(|state| state.library.items.len() == 2));
    let red = state.library.items.values().find(|item| item.file_name() == "red.png").unwrap().clone();
    assert_eq!(red.kind, MediaKind::Image);

    let thumbnail = client.thumbnail(red.id).unwrap();
    assert!(thumbnail.starts_with(&[0xff, 0xd8]), "a JPEG");
    let original = std::fs::read(pictures.path().join("red.png")).unwrap();
    assert_eq!(client.file(red.id).unwrap(), original);
    let mut file = RemoteFile::open(client.clone(), red.id).unwrap();
    assert_eq!(file.len(), original.len() as u64);
    std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(10)).unwrap();
    let mut rest = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut rest).unwrap();
    assert_eq!(rest, original[10..]);

    let mut library = state.library;
    let before = library.snapshot();
    library.add_item_tags(&[red.id], &["warm".to_string()]);
    let patch = library.diff(&before);
    let version = client.send_patch(state.version, &patch).unwrap();
    assert!(version > state.version);
    assert!(
        matches!(client.send_patch(state.version, &patch), Err(RemoteError::Conflict)),
        "made against an old version, changing what changed since"
    );
    let blue = library.items.values().find(|item| item.file_name() == "blue.png").unwrap().id;
    let before = library.snapshot();
    library.add_item_tags(&[blue], &["cool".to_string()]);
    let elsewhere = library.diff(&before);
    let rebased = client.send_patch(state.version, &elsewhere).unwrap();
    assert!(rebased > version, "made against an old version, but touching nothing that changed since");
    let version = rebased;
    let fetched = client.library().unwrap();
    assert!(fetched.library.item(red.id).unwrap().tags.contains("warm"), "the edit is kept on the server");

    let mut moved = fetched.library.item(red.id).unwrap().clone();
    moved.path = "/etc/passwd".into();
    let sneaky = sortier_library::patch::Patch {
        items: vec![moved],
        next_id: patch.next_id,
        ..Default::default()
    };
    assert!(matches!(client.send_patch(fetched.version, &sneaky), Err(RemoteError::Server(_))), "files can't be moved");

    let cover = client.cover_from_page(red.id, 0).unwrap();
    assert!(client.cover(&cover).unwrap().starts_with(&[0xff, 0xd8]));

    let upload = pictures.path().join("green.png");
    std::fs::write(&upload, png([30, 200, 30])).unwrap();
    let staged = tempfile::tempdir().unwrap();
    let local = staged.path().join("green.png");
    std::fs::rename(&upload, &local).unwrap();
    client.upload("uploads/green.png", &local, |_| {}).unwrap();
    assert!(client.upload("../escape.png", &local, |_| {}).is_err());
    let changes = wait_for("the upload to be scanned", || {
        let changes = client.changes(version).ok()?;
        let library = client.library().ok()?;
        library.library.items.values().any(|item| item.file_name() == "green.png").then_some(changes)
    });
    assert!(changes.version > version);
    assert!(pictures.path().join("uploads/green.png").is_file());
}
