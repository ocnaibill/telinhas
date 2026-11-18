//! The updater against a small HTTP server on this machine.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};
use telinhas_update::{Error, check, download};

/// Serves the given `(path, body)` pairs until dropped.
struct Server {
    base: String,
    stop: Arc<AtomicBool>,
}

impl Server {
    fn start(files: Vec<(&'static str, Vec<u8>)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                let mut request = [0u8; 2048];
                let n = stream.read(&mut request).unwrap_or(0);
                let text = String::from_utf8_lossy(&request[..n]);
                let path = text.split_whitespace().nth(1).unwrap_or("/");
                match files.iter().find(|(p, _)| *p == path) {
                    Some((_, body)) => {
                        let head = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(head.as_bytes());
                        let _ = stream.write_all(body);
                    }
                    None => {
                        let _ = stream.write_all(
                            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                    }
                }
            }
        });
        Self { base, stop }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn manifest(version: &str, installer: &str, hash: &str) -> Vec<u8> {
    format!(r#"{{"version":"{version}","installer":"{installer}","sha256":"{hash}","notes":"novidades"}}"#)
        .into_bytes()
}

const WAIT: Duration = Duration::from_secs(5);

fn scratch(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("telinhas-update-{name}-{}", std::process::id()))
}

#[test]
fn a_newer_version_is_found_downloaded_and_checked() {
    let installer = b"pretend this is an installer".to_vec();
    let server = Server::start(vec![
        (
            "/dl/latest.json",
            manifest("0.2.0", "Telinhas-Setup-0.2.0.exe", &sha256(&installer)),
        ),
        ("/dl/Telinhas-Setup-0.2.0.exe", installer.clone()),
    ]);
    let update = check(&format!("{}/dl/latest.json", server.base), "0.1.0", WAIT)
        .unwrap()
        .expect("0.2.0 is newer than 0.1.0");
    assert_eq!(update.version.to_string(), "0.2.0");
    assert_eq!(update.notes, "novidades");

    let dir = scratch("ok");
    let mut last = (0, None);
    let path = download(&update, &dir, WAIT, &mut |done, total| last = (done, total)).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), installer);
    assert_eq!(last, (installer.len() as u64, Some(installer.len() as u64)));
    assert!(!dir.join("Telinhas-Setup-0.2.0.exe.part").exists());
}

#[test]
fn the_same_or_an_older_version_is_left_alone() {
    let server = Server::start(vec![(
        "/dl/latest.json",
        manifest("0.1.0", "x.exe", &sha256(b"x")),
    )]);
    let url = format!("{}/dl/latest.json", server.base);
    assert!(check(&url, "0.1.0", WAIT).unwrap().is_none());
    assert!(check(&url, "0.3.0", WAIT).unwrap().is_none());
    // A pre-release is older than the release it leads to.
    assert!(check(&url, "0.1.0-beta.1", WAIT).unwrap().is_some());
}

#[test]
fn a_download_that_does_not_match_its_hash_is_thrown_away() {
    let server = Server::start(vec![
        (
            "/dl/latest.json",
            manifest(
                "0.2.0",
                "Telinhas-Setup-0.2.0.exe",
                &sha256(b"what the manifest promised"),
            ),
        ),
        ("/dl/Telinhas-Setup-0.2.0.exe", b"something else".to_vec()),
    ]);
    let update = check(&format!("{}/dl/latest.json", server.base), "0.1.0", WAIT)
        .unwrap()
        .unwrap();
    let dir = scratch("bad");
    let error = download(&update, &dir, WAIT, &mut |_, _| {}).unwrap_err();
    assert!(matches!(error, Error::Integrity(_)), "{error}");
    let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
    assert!(
        leftovers.is_empty(),
        "nothing may stay behind: {leftovers:?}"
    );
}

#[test]
fn a_missing_or_broken_manifest_is_an_error_not_a_crash() {
    let server = Server::start(vec![("/dl/broken.json", b"{not json".to_vec())]);
    let base = &server.base;
    assert!(matches!(
        check(&format!("{base}/dl/nope.json"), "0.1.0", WAIT),
        Err(Error::Network(_))
    ));
    assert!(matches!(
        check(&format!("{base}/dl/broken.json"), "0.1.0", WAIT),
        Err(Error::Manifest(_))
    ));
    // Nothing is listening: the app must just go on without updating.
    assert!(
        check(
            "http://127.0.0.1:1/dl/latest.json",
            "0.1.0",
            Duration::from_secs(2)
        )
        .is_err()
    );
}

#[test]
fn a_badly_formed_hash_is_refused_before_downloading() {
    let server = Server::start(vec![(
        "/dl/latest.json",
        manifest("0.2.0", "x.exe", "not-a-hash"),
    )]);
    let error = check(&format!("{}/dl/latest.json", server.base), "0.1.0", WAIT).unwrap_err();
    assert!(matches!(error, Error::Manifest(_)), "{error}");
}
