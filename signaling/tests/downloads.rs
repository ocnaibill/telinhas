//! The `/dl` folder the app updates itself from.

use telinhas_signaling::{Config, Hub, serve_with_downloads};
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
