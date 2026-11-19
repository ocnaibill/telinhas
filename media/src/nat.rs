//! A check of what kind of NAT the connection sits behind, for the
//! diagnostics. Two STUN servers are asked, from the same socket, which public
//! address they see. A NAT that gives the same port to both (endpoint
//! independent mapping) lets two peers reach each other directly; one that
//! gives a different port for each destination (symmetric) does not, and needs
//! a relay.

use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

const SERVERS: [&str; 2] = ["stun.l.google.com:19302", "stun.cloudflare.com:3478"];
const COOKIE: u32 = 0x2112_A442;
const ATTEMPTS: u32 = 3;
const WAIT: Duration = Duration::from_millis(700);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The same public port for both servers: direct connections can work.
    Friendly,
    /// A different public port for each server: direct connections fail.
    Symmetric,
    /// Only one server answered, so the two could not be compared.
    Inconclusive,
    /// Nobody answered: UDP to the internet may be blocked.
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NatReport {
    pub verdict: Verdict,
    /// What each server saw, for the diagnostics.
    pub detail: String,
}

/// Asks the two public STUN servers. Takes a few seconds in the worst case:
/// run it off the interface's thread.
pub fn check() -> NatReport {
    let addrs: Vec<(String, Option<SocketAddr>)> = SERVERS
        .iter()
        .map(|name| {
            let addr = name
                .to_socket_addrs()
                .ok()
                .and_then(|mut all| all.find(SocketAddr::is_ipv4));
            ((*name).to_owned(), addr)
        })
        .collect();
    let report = check_servers(&addrs);
    crate::log::line(&format!("NAT {:?}: {}", report.verdict, report.detail));
    report
}

fn check_servers(servers: &[(String, Option<SocketAddr>)]) -> NatReport {
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
        return NatReport {
            verdict: Verdict::Blocked,
            detail: "não foi possível abrir um socket UDP".to_owned(),
        };
    };
    let _ = socket.set_read_timeout(Some(WAIT));
    let seen: Vec<(String, Option<SocketAddr>)> = servers
        .iter()
        .map(|(name, addr)| (name.clone(), addr.and_then(|addr| ask(&socket, addr))))
        .collect();
    let detail = seen
        .iter()
        .map(|(name, mapped)| match mapped {
            Some(mapped) => format!("{name} vê {mapped}"),
            None => format!("{name} não respondeu"),
        })
        .collect::<Vec<_>>()
        .join("; ");
    let answered: Vec<SocketAddr> = seen.iter().filter_map(|(_, m)| *m).collect();
    let verdict = match answered.as_slice() {
        [] => Verdict::Blocked,
        [_] => Verdict::Inconclusive,
        [first, rest @ ..] => {
            if rest.iter().all(|other| other.port() == first.port()) {
                Verdict::Friendly
            } else {
                Verdict::Symmetric
            }
        }
    };
    NatReport { verdict, detail }
}

/// One STUN binding request: the public address the server saw.
fn ask(socket: &UdpSocket, server: SocketAddr) -> Option<SocketAddr> {
    let id: [u8; 12] = rand::random();
    let mut request = Vec::with_capacity(20);
    request.extend_from_slice(&0x0001u16.to_be_bytes());
    request.extend_from_slice(&0u16.to_be_bytes());
    request.extend_from_slice(&COOKIE.to_be_bytes());
    request.extend_from_slice(&id);
    let mut buffer = [0u8; 512];
    for _ in 0..ATTEMPTS {
        socket.send_to(&request, server).ok()?;
        let started = Instant::now();
        while started.elapsed() < WAIT {
            let Ok((length, from)) = socket.recv_from(&mut buffer) else {
                break;
            };
            if from == server
                && let Some(mapped) = parse_response(&buffer[..length], &id)
            {
                return Some(mapped);
            }
        }
    }
    None
}

/// The address in a binding success response to the request `id`.
fn parse_response(data: &[u8], id: &[u8; 12]) -> Option<SocketAddr> {
    if data.len() < 20 || data[..2] != 0x0101u16.to_be_bytes() || data[8..20] != id[..] {
        return None;
    }
    let mut at = 20;
    while at + 4 <= data.len() {
        let kind = u16::from_be_bytes([data[at], data[at + 1]]);
        let length = usize::from(u16::from_be_bytes([data[at + 2], data[at + 3]]));
        let value = data.get(at + 4..at + 4 + length)?;
        // XOR-MAPPED-ADDRESS, or the old MAPPED-ADDRESS, for IPv4.
        if (kind == 0x0020 || kind == 0x0001) && length >= 8 && value[1] == 1 {
            let (mut port, mut ip) = (
                u16::from_be_bytes([value[2], value[3]]),
                u32::from_be_bytes([value[4], value[5], value[6], value[7]]),
            );
            if kind == 0x0020 {
                port ^= (COOKIE >> 16) as u16;
                ip ^= COOKIE;
            }
            return Some(SocketAddr::from((ip.to_be_bytes(), port)));
        }
        at += 4 + length.div_ceil(4) * 4;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A STUN server on this machine that reports `port` for every request
    /// (or the real one with `None`), until the test ends.
    fn fake_server(port: Option<u16>) -> SocketAddr {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = socket.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 64];
            while let Ok((length, from)) = socket.recv_from(&mut buffer) {
                if length < 20 {
                    continue;
                }
                let seen = port.unwrap_or(from.port()) ^ (COOKIE >> 16) as u16;
                let mut reply = Vec::new();
                reply.extend_from_slice(&0x0101u16.to_be_bytes());
                reply.extend_from_slice(&12u16.to_be_bytes());
                reply.extend_from_slice(&buffer[4..20]);
                reply.extend_from_slice(&0x0020u16.to_be_bytes());
                reply.extend_from_slice(&8u16.to_be_bytes());
                reply.extend_from_slice(&[0, 1]);
                reply.extend_from_slice(&seen.to_be_bytes());
                reply.extend_from_slice(&(0x7F00_0001u32 ^ COOKIE).to_be_bytes());
                let _ = socket.send_to(&reply, from);
            }
        });
        address
    }

    fn named(addr: SocketAddr, name: &str) -> (String, Option<SocketAddr>) {
        (name.to_owned(), Some(addr))
    }

    #[test]
    fn the_same_port_from_two_servers_is_friendly() {
        let report = check_servers(&[
            named(fake_server(Some(40000)), "a"),
            named(fake_server(Some(40000)), "b"),
        ]);
        assert_eq!(report.verdict, Verdict::Friendly, "{}", report.detail);
        assert!(report.detail.contains("127.0.0.1:40000"));
    }

    #[test]
    fn a_different_port_for_each_server_is_symmetric() {
        let report = check_servers(&[
            named(fake_server(Some(40000)), "a"),
            named(fake_server(Some(40001)), "b"),
        ]);
        assert_eq!(report.verdict, Verdict::Symmetric);
    }

    #[test]
    fn one_answer_is_not_enough_and_none_means_blocked() {
        let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
        let silent_addr = silent.local_addr().unwrap();
        let one = check_servers(&[named(fake_server(None), "a"), named(silent_addr, "b")]);
        assert_eq!(one.verdict, Verdict::Inconclusive);
        assert!(one.detail.contains("b não respondeu"));
        let none = check_servers(&[named(silent_addr, "a"), ("b".to_owned(), None)]);
        assert_eq!(none.verdict, Verdict::Blocked);
    }

    #[test]
    fn answers_for_another_request_are_ignored() {
        let mut reply = vec![0x01, 0x01, 0, 0, 0x21, 0x12, 0xA4, 0x42];
        reply.extend_from_slice(&[9u8; 12]);
        assert_eq!(parse_response(&reply, &[1u8; 12]), None);
        assert_eq!(parse_response(&[0u8; 3], &[1u8; 12]), None);
    }

    /// Needs the internet: `cargo test -p telinhas-media real_servers -- --ignored --nocapture`.
    #[test]
    #[ignore = "asks the public STUN servers"]
    fn real_servers() {
        let report = check();
        println!("{:?}: {}", report.verdict, report.detail);
        assert_ne!(report.verdict, Verdict::Blocked);
    }
}
