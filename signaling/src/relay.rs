//! Relay (TURN) credentials from Cloudflare, for the people whose network
//! cannot reach another person directly (a symmetric NAT or CGNAT).
//!
//! The key id and token stay on the server, read from the environment and never
//! printed. Clients only get short-lived credentials, inside `room_joined`.
//! One set is shared by everyone and replaced well before it expires.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::hub::Hub;
use crate::protocol::IceServer;

/// How long a credential set lives, and how often it is replaced: someone who
/// joins right before a refresh still has about 20 hours.
const TTL_SECONDS: u64 = 86_400;
const REFRESH: Duration = Duration::from_secs(4 * 3600);
const RETRY: Duration = Duration::from_secs(60);

/// The relay entries of Cloudflare's answer that this app can use: the ones
/// with credentials, over UDP on port 3478. (Port 443 over UDP never answered
/// in tests and held up the whole gathering of addresses; a STUN-only entry has
/// no credentials, and the clients already have their own STUN servers.)
pub fn parse_ice_servers(answer: &Value) -> Vec<IceServer> {
    let Some(list) = answer.get("iceServers").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|entry| {
            let username = entry.get("username")?.as_str()?.to_owned();
            let credential = entry.get("credential")?.as_str()?.to_owned();
            let urls: Vec<String> = entry
                .get("urls")?
                .as_array()?
                .iter()
                .filter_map(Value::as_str)
                .filter(|url| {
                    url.starts_with("turn:")
                        && url.contains(":3478")
                        && url.ends_with("transport=udp")
                })
                .map(str::to_owned)
                .collect();
            (!urls.is_empty() && !username.is_empty()).then_some(IceServer {
                urls,
                username,
                credential,
            })
        })
        .collect()
}

/// Asks Cloudflare for a credential set. Blocking: call it off the async threads.
pub fn fetch(key_id: &str, token: &str) -> Result<Vec<IceServer>, String> {
    let url = format!(
        "https://rtc.live.cloudflare.com/v1/turn/keys/{key_id}/credentials/generate-ice-servers"
    );
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into();
    let mut response = agent
        .post(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .send_json(json!({ "ttl": TTL_SECONDS }))
        // The URL holds the key id: say what failed, not where.
        .map_err(|error| format!("the request failed ({})", short(&error.to_string())))?;
    let answer: Value = response.body_mut().read_json().map_err(|error| {
        format!(
            "the answer could not be read ({})",
            short(&error.to_string())
        )
    })?;
    let servers = parse_ice_servers(&answer);
    if servers.is_empty() {
        return Err("the answer had no relay in it".to_owned());
    }
    Ok(servers)
}

/// Keeps only the start of an error, so a long one cannot carry a secret along.
fn short(text: &str) -> String {
    text.chars().take(60).collect()
}

/// Keeps the hub supplied with fresh relay credentials, until the process ends.
pub fn keep_fresh(hub: Arc<Hub>, key_id: String, token: String) {
    tokio::spawn(async move {
        loop {
            let (id, secret) = (key_id.clone(), token.clone());
            let wait = match tokio::task::spawn_blocking(move || fetch(&id, &secret)).await {
                Ok(Ok(servers)) => {
                    println!(
                        "relay: credentials renewed ({} server entries)",
                        servers.len()
                    );
                    hub.set_ice_servers(servers);
                    REFRESH
                }
                Ok(Err(reason)) => {
                    eprintln!("relay: {reason}; trying again in a minute");
                    RETRY
                }
                Err(_) => RETRY,
            };
            tokio::time::sleep(wait).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_relays_with_credentials_over_udp_are_kept() {
        let answer = json!({ "iceServers": [
            { "urls": ["stun:stun.cloudflare.com:3478"] },
            {
                "urls": [
                    "turn:turn.cloudflare.com:3478?transport=udp",
                    "turn:turn.cloudflare.com:3478?transport=tcp",
                    "turns:turn.cloudflare.com:5349?transport=tcp",
                    "turn:turn.cloudflare.com:443?transport=udp"
                ],
                "username": "user",
                "credential": "secret"
            }
        ]});
        assert_eq!(
            parse_ice_servers(&answer),
            vec![IceServer {
                urls: vec!["turn:turn.cloudflare.com:3478?transport=udp".into()],
                username: "user".into(),
                credential: "secret".into(),
            }]
        );
    }

    #[test]
    fn nonsense_gives_no_relays() {
        assert!(parse_ice_servers(&json!({})).is_empty());
        assert!(parse_ice_servers(&json!({ "iceServers": "x" })).is_empty());
        assert!(
            parse_ice_servers(
                &json!({ "iceServers": [{ "urls": [], "username": "u", "credential": "c" }] })
            )
            .is_empty()
        );
    }
}
