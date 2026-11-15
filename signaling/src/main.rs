use telinhas_signaling::{Config, Hub, PROTOCOL_VERSION, serve};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let bind = std::env::var("TELINHAS_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    println!(
        "telinhas-signaling (protocol v{PROTOCOL_VERSION}) listening on {}",
        listener.local_addr()?
    );
    serve(listener, Hub::new(Config::default())).await
}
