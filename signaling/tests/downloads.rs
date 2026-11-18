//! The `/dl` folder the app updates itself from.

use telinhas_signaling::{Config, Hub, serve_with_downloads, serve_with_files};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn start(downloads: Option<std::path::PathBuf>) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve_with_downloads(
        listener,
        Hub::new(Config::default()),
        downloads,
    ));
    addr
}

/// One plain HTTP/1.1 request; returns the status line and the whole reply.
async fn get(addr: std::net::SocketAddr, path: &str) -> (String, String) {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).await.unwrap();
    let status = reply.lines().next().unwrap_or_default().to_owned();
    (status, reply)
}

fn folder(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("telinhas-dl-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("latest.json"), r#"{"version":"1.2.3"}"#).unwrap();
    std::fs::write(dir.join("secret.txt"), "not in the folder").unwrap();
    dir
}

#[tokio::test]
async fn files_of_the_folder_are_served_and_never_cached() {
    let dir = folder("serve");
    let inside = dir.join("pub");
    std::fs::create_dir_all(&inside).unwrap();
    std::fs::write(inside.join("Telinhas-Setup.exe"), b"MZ-installer").unwrap();
    let addr = start(Some(inside)).await;

    let (status, reply) = get(addr, "/dl/Telinhas-Setup.exe").await;
    assert!(status.contains("200"), "{status}");
    assert!(reply.contains("MZ-installer"));
    assert!(
        reply.to_lowercase().contains("cache-control: no-cache"),
        "{reply}"
    );
    // Nothing above the folder, however it is asked for.
    for path in [
        "/dl/../secret.txt",
        "/dl/%2e%2e/secret.txt",
        "/dl/..%2fsecret.txt",
    ] {
        let (status, reply) = get(addr, path).await;
        assert!(
            !reply.contains("not in the folder"),
            "{path} leaked: {reply}"
        );
        assert!(!status.contains("200"), "{path}: {status}");
    }
    // No listing of the folder.
    let (_, reply) = get(addr, "/dl/").await;
    assert!(!reply.contains("Telinhas-Setup.exe"));
}

#[tokio::test]
async fn without_a_folder_there_is_no_dl() {
    let addr = start(None).await;
    let (status, _) = get(addr, "/dl/latest.json").await;
    assert!(status.contains("404"), "{status}");
    // The rest keeps working.
    let (status, reply) = get(addr, "/healthz").await;
    assert!(status.contains("200") && reply.ends_with("ok"), "{reply}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_updater_finds_and_downloads_a_release_from_this_server() {
    use sha2::{Digest, Sha256};
    let dir = folder("release");
    let release = dir.join("release");
    std::fs::create_dir_all(&release).unwrap();
    // Big enough to arrive in several pieces.
    let installer: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    let hash: String = Sha256::digest(&installer)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    std::fs::write(release.join("Telinhas-Setup-0.2.0.exe"), &installer).unwrap();
    std::fs::write(
        release.join("latest.json"),
        format!(r#"{{"version":"0.2.0","installer":"Telinhas-Setup-0.2.0.exe","sha256":"{hash}","notes":"teste"}}"#),
    )
    .unwrap();
    let addr = start(Some(release)).await;

    let url = format!("http://{addr}/dl/latest.json");
    let target = dir.join("downloaded");
    let (found, saved) = tokio::task::spawn_blocking(move || {
        let timeout = std::time::Duration::from_secs(10);
        let update = telinhas_update::check(&url, "0.1.0", timeout)
            .unwrap()
            .unwrap();
        let mut last = 0;
        let path = telinhas_update::download(&update, &target, timeout, &mut |done, _| last = done)
            .unwrap();
        assert_eq!(last, 3_000_000);
        (update.version.to_string(), std::fs::read(path).unwrap())
    })
    .await
    .unwrap();
    assert_eq!(found, "0.2.0");
    assert_eq!(saved, installer);
}

#[tokio::test]
async fn the_site_is_served_at_the_root_without_hiding_the_rest() {
    let dir = folder("site");
    let site = dir.join("site");
    let dl = dir.join("dl");
    std::fs::create_dir_all(&site).unwrap();
    std::fs::create_dir_all(&dl).unwrap();
    std::fs::write(site.join("index.html"), "<h1>Telinhas</h1>").unwrap();
    std::fs::write(dl.join("latest.json"), r#"{"version":"0.1.0"}"#).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve_with_files(
        listener,
        Hub::new(Config::default()),
        Some(dl),
        Some(site),
    ));

    let (status, reply) = get(addr, "/").await;
    assert!(
        status.contains("200") && reply.contains("<h1>Telinhas</h1>"),
        "{reply}"
    );
    let (status, reply) = get(addr, "/dl/latest.json").await;
    assert!(status.contains("200") && reply.contains("0.1.0"), "{reply}");
    let (status, reply) = get(addr, "/healthz").await;
    assert!(status.contains("200") && reply.ends_with("ok"), "{reply}");
    let (status, _) = get(addr, "/nada-aqui").await;
    assert!(status.contains("404"), "{status}");
    let (_, reply) = get(addr, "/../secret.txt").await;
    assert!(!reply.contains("not in the folder"));
}
