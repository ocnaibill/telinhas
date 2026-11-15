use telinhas_signaling::{Config, Hub, PROTOCOL_VERSION, parse_ip_list, serve};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let bind = std::env::var("TELINHAS_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());

    let mut config = Config::default();
    // Peers allowed to say who the real client is (`CF-Connecting-IP`):
    // the machine where cloudflared runs. Defaults to loopback only.
    if let Ok(list) = std::env::var("TELINHAS_TRUSTED_PROXIES") {
        config.trusted_proxies = parse_ip_list(&list)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    }

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    println!(
        "telinhas-signaling (protocol v{PROTOCOL_VERSION}) listening on {}",
        listener.local_addr()?
    );
    println!("trusted proxies: {:?}", config.trusted_proxies);
    serve(listener, Hub::new(config)).await
}
