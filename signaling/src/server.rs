//! HTTP and WebSocket front end.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::header::CACHE_CONTROL;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::IntoResponse;
use axum::routing::get;
use tokio::sync::mpsc;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::hub::{Config, Hub, Out};
use crate::protocol::{ErrorCode, encode};

/// Builds the router: `/ws` for clients and `/healthz` for the tunnel.
pub fn router(hub: Arc<Hub>) -> Router {
    Router::new()
        .route("/ws", get(ws_handler))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(hub)
}

/// The same router, plus `/dl` serving the files of a folder (read only):
/// where the app finds `latest.json` and the installer to update itself. No
/// folder means no `/dl`. The server writes nothing to disk (RNF-20).
pub fn router_with_downloads(hub: Arc<Hub>, downloads: Option<PathBuf>) -> Router {
    let router = router(hub);
    match downloads {
        Some(dir) => router.merge(
            Router::new()
                .nest_service("/dl", ServeDir::new(dir))
                // The app must see a new version as soon as it is published.
                .layer(SetResponseHeaderLayer::overriding(
                    CACHE_CONTROL,
                    HeaderValue::from_static("no-cache"),
                )),
        ),
        None => router,
    }
}

/// Everything, plus a site at `/` from a folder (the landing page). Paths the
/// server already answers (`/ws`, `/healthz`, `/dl`) are not affected.
pub fn router_with_files(
    hub: Arc<Hub>,
    downloads: Option<PathBuf>,
    site: Option<PathBuf>,
) -> Router {
    let router = router_with_downloads(hub, downloads);
    match site {
        Some(dir) => router.fallback_service(ServeDir::new(dir)),
        None => router,
    }
}

/// Serves until the listener fails.
pub async fn serve(listener: tokio::net::TcpListener, hub: Arc<Hub>) -> std::io::Result<()> {
    serve_with_downloads(listener, hub, None).await
}

/// Like [`serve`], with the downloads folder of [`router_with_downloads`].
pub async fn serve_with_downloads(
    listener: tokio::net::TcpListener,
    hub: Arc<Hub>,
    downloads: Option<PathBuf>,
) -> std::io::Result<()> {
    serve_with_files(listener, hub, downloads, None).await
}

/// Like [`serve`], with the downloads folder and the site of [`router_with_files`].
pub async fn serve_with_files(
    listener: tokio::net::TcpListener,
    hub: Arc<Hub>,
    downloads: Option<PathBuf>,
    site: Option<PathBuf>,
) -> std::io::Result<()> {
    axum::serve(
        listener,
        router_with_files(hub, downloads, site).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
}

/// The client address. `CF-Connecting-IP` counts only when the socket peer is
/// a trusted proxy, such as the local Cloudflare Tunnel (DEC-48).
fn client_ip(cfg: &Config, peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
    if cfg.trusted_proxies.contains(&peer.ip())
        && let Some(ip) = headers
            .get("cf-connecting-ip")
            .and_then(|value| value.to_str().ok())
            .and_then(|text| text.trim().parse().ok())
    {
        return ip;
    }
    peer.ip()
}

async fn ws_handler(
    State(hub): State<Arc<Hub>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    let ip = client_ip(hub.config(), peer, &headers);
    let max = hub.config().max_message_bytes;
    upgrade
        .max_message_size(max)
        .max_frame_size(max)
        .on_upgrade(move |socket| run_socket(hub, socket, ip))
}

async fn run_socket(hub: Arc<Hub>, mut socket: WebSocket, ip: IpAddr) {
    let (tx, mut outgoing) = mpsc::unbounded_channel::<Out>();
    let conn = hub.register(ip, tx);
    let cfg = hub.config().clone();

    let mut ping = tokio::time::interval(cfg.ping_interval);
    let mut last_seen = Instant::now();
    let mut window_start = Instant::now();
    let mut in_window = 0u32;

    loop {
        tokio::select! {
            incoming = socket.recv() => {
                let Some(Ok(frame)) = incoming else { break };
                last_seen = Instant::now();
                match frame {
                    Message::Text(text) => {
                        if window_start.elapsed().as_secs() >= 1 {
                            window_start = Instant::now();
                            in_window = 0;
                        }
                        in_window += 1;
                        if in_window > cfg.max_messages_per_second {
                            hub.reject(conn, ErrorCode::BadMessage, "too many messages");
                            hub.close(conn);
                            continue;
                        }
                        hub.handle_text(conn, text.as_str());
                    }
                    Message::Binary(_) => {
                        hub.reject(conn, ErrorCode::BadMessage, "binary frames are not supported");
                    }
                    Message::Close(_) => break,
                    Message::Ping(_) | Message::Pong(_) => {}
                }
            }
            out = outgoing.recv() => match out {
                Some(Out::Message(message)) => {
                    if socket.send(Message::Text(encode(&message).into())).await.is_err() {
                        break;
                    }
                }
                Some(Out::Close) => {
                    let _ = socket.send(Message::Close(None)).await;
                    break;
                }
                None => break,
            },
            _ = ping.tick() => {
                if last_seen.elapsed() > cfg.idle_timeout
                    || socket.send(Message::Ping(Bytes::new())).await.is_err()
                {
                    break;
                }
            }
        }
    }
    hub.disconnect(conn);
}
