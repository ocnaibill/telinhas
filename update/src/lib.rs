//! Self-update.
//!
//! The app looks for a `latest.json` at a fixed address (the homelab's `/dl`),
//! and when it names a newer version it downloads the installer, checks its
//! SHA-256 and runs it silently. The installer replaces the files and starts the
//! app again.
//!
//! What this protects against: a corrupted or truncated download, and a
//! manifest that points the installer somewhere else. It does not protect
//! against whoever controls the server publishing a bad installer: the hash
//! comes from the same place as the file. A signature would close that.

use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The installer is about 100 MB at most (RNF-13); anything much bigger is wrong.
const MAX_INSTALLER_BYTES: u64 = 400 * 1024 * 1024;

#[derive(Debug)]
pub enum Error {
    /// The server could not be reached or answered with an error.
    Network(String),
    /// The manifest is not what it should be.
    Manifest(String),
    /// The downloaded file does not match the manifest.
    Integrity(String),
    Io(String),
    Unsupported,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Network(m) => write!(f, "sem conexão com o servidor de atualização: {m}"),
            Error::Manifest(m) => write!(f, "arquivo de versão inválido: {m}"),
            Error::Integrity(m) => write!(f, "o download não confere: {m}"),
            Error::Io(m) => write!(f, "erro de arquivo: {m}"),
            Error::Unsupported => write!(f, "a atualização automática só existe no Windows"),
        }
    }
}

impl std::error::Error for Error {}

/// `latest.json`, as the release script writes it.
#[derive(Debug, Deserialize)]
struct Manifest {
    version: String,
    /// A file name next to the manifest, or a full address on the same server.
    installer: String,
    /// SHA-256 of the installer, in hex.
    sha256: String,
    #[serde(default)]
    notes: String,
}

/// A newer version, ready to be downloaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub version: Version,
    pub installer_url: String,
    pub sha256: String,
    pub notes: String,
}

/// Only HTTPS, except for a server on this machine (tests, development).
fn allowed(url: &str) -> bool {
    url.starts_with("https://")
        || url.starts_with("http://127.0.0.1")
        || url.starts_with("http://localhost")
}

/// `https://host` of an address, to compare where two files come from.
fn origin(url: &str) -> &str {
    let rest = url.find("://").map_or(0, |i| i + 3);
    let end = url[rest..].find('/').map_or(url.len(), |i| rest + i);
    &url[..end]
}

/// The installer's address: as given when absolute, next to the manifest when
/// just a file name.
fn resolve(manifest_url: &str, installer: &str) -> Result<String, Error> {
    let url = if installer.contains("://") {
        installer.to_owned()
    } else if installer.contains('/') || installer.contains('\\') || installer.contains("..") {
        return Err(Error::Manifest(format!(
            "nome de instalador estranho: {installer}"
        )));
    } else {
        let base = manifest_url
            .rsplit_once('/')
            .map_or(manifest_url, |(base, _)| base);
        format!("{base}/{installer}")
    };
    if !allowed(&url) {
        return Err(Error::Manifest("o instalador não vem por HTTPS".to_owned()));
    }
    // A manifest cannot send the app to download from somewhere else.
    if origin(&url) != origin(manifest_url) {
        return Err(Error::Manifest(
            "o instalador está em outro servidor".to_owned(),
        ));
    }
    Ok(url)
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .build()
        .into()
}

/// Asks the server for the latest version. `None` when this one is current or
/// newer.
pub fn check(
    manifest_url: &str,
    current: &str,
    timeout: Duration,
) -> Result<Option<Update>, Error> {
    if !allowed(manifest_url) {
        return Err(Error::Manifest(
            "o endereço de atualização não é HTTPS".to_owned(),
        ));
    }
    let current = Version::parse(current)
        .map_err(|e| Error::Manifest(format!("versão atual {current:?}: {e}")))?;
    let mut response = agent(timeout)
        .get(manifest_url)
        .call()
        .map_err(|e| Error::Network(e.to_string()))?;
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|e| Error::Network(e.to_string()))?;
    // Windows tools like to start a text file with a byte order mark.
    let manifest: Manifest = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| Error::Manifest(e.to_string()))?;
    let latest = Version::parse(manifest.version.trim_start_matches('v'))
        .map_err(|e| Error::Manifest(format!("versão {:?}: {e}", manifest.version)))?;
    if latest <= current {
        return Ok(None);
    }
    let sha256 = manifest.sha256.trim().to_lowercase();
    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Manifest(
            "o hash SHA-256 está mal formado".to_owned(),
        ));
    }
    Ok(Some(Update {
        installer_url: resolve(manifest_url, &manifest.installer)?,
        version: latest,
        sha256,
        notes: manifest.notes,
    }))
}

/// Downloads the installer into `dir` and checks it. `progress` gets the bytes
/// so far and the total, when the server says it. The file only gets its final
/// name once its hash matches.
pub fn download(
    update: &Update,
    dir: &Path,
    timeout: Duration,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<PathBuf, Error> {
    std::fs::create_dir_all(dir).map_err(|e| Error::Io(e.to_string()))?;
    let name = format!("Telinhas-Setup-{}.exe", update.version);
    let partial = dir.join(format!("{name}.part"));
    let target = dir.join(name);

    let response = agent(timeout)
        .get(&update.installer_url)
        .call()
        .map_err(|e| Error::Network(e.to_string()))?;
    let total = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if total.is_some_and(|t| t > MAX_INSTALLER_BYTES) {
        return Err(Error::Integrity("o instalador é grande demais".to_owned()));
    }

    let mut reader = response.into_body().into_reader();
    let mut file = std::fs::File::create(&partial).map_err(|e| Error::Io(e.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    let result = (|| -> Result<(), Error> {
        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|e| Error::Network(e.to_string()))?;
            if read == 0 {
                break;
            }
            done += read as u64;
            if done > MAX_INSTALLER_BYTES {
                return Err(Error::Integrity("o instalador é grande demais".to_owned()));
            }
            hasher.update(&buffer[..read]);
            file.write_all(&buffer[..read])
                .map_err(|e| Error::Io(e.to_string()))?;
            progress(done, total);
        }
        file.flush().map_err(|e| Error::Io(e.to_string()))?;
        Ok(())
    })();
    drop(file);
    if let Err(error) = result {
        let _ = std::fs::remove_file(&partial);
        return Err(error);
    }
    if total.is_some_and(|t| t != done) {
        let _ = std::fs::remove_file(&partial);
        return Err(Error::Integrity("o download veio incompleto".to_owned()));
    }
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if digest != update.sha256 {
        let _ = std::fs::remove_file(&partial);
        return Err(Error::Integrity(format!(
            "esperava {}, veio {digest}",
            update.sha256
        )));
    }
    std::fs::rename(&partial, &target).map_err(|e| Error::Io(e.to_string()))?;
    Ok(target)
}

/// Starts the installer silently, apart from this process, so the app can
/// close while it works. The installer starts the app again when it is done.
#[cfg(windows)]
pub fn run_installer(path: &Path) -> Result<(), Error> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    std::process::Command::new(path)
        .args([
            "/VERYSILENT",
            "/SUPPRESSMSGBOXES",
            "/NORESTART",
            "/FORCECLOSEAPPLICATIONS",
        ])
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .map(|_| ())
        .map_err(|e| Error::Io(e.to_string()))
}

#[cfg(not(windows))]
pub fn run_installer(_path: &Path) -> Result<(), Error> {
    Err(Error::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_installer_is_resolved_next_to_the_manifest() {
        let base = "https://example.org/dl/latest.json";
        assert_eq!(
            resolve(base, "Telinhas-Setup-1.2.3.exe").unwrap(),
            "https://example.org/dl/Telinhas-Setup-1.2.3.exe"
        );
        assert!(resolve(base, "https://example.org/other/x.exe").is_ok());
    }

    #[test]
    fn a_manifest_cannot_point_somewhere_else_or_outside_the_folder() {
        let base = "https://example.org/dl/latest.json";
        assert!(resolve(base, "https://evil.example/x.exe").is_err());
        assert!(resolve(base, "../x.exe").is_err());
        assert!(resolve(base, "sub/x.exe").is_err());
        assert!(
            resolve(base, "http://example.org/dl/x.exe").is_err(),
            "plain HTTP"
        );
        assert!(resolve("http://127.0.0.1:9/dl/latest.json", "x.exe").is_ok());
    }

    #[test]
    fn only_https_or_this_machine_is_allowed() {
        assert!(allowed("https://example.org/a"));
        assert!(allowed("http://127.0.0.1:8080/a"));
        assert!(allowed("http://localhost/a"));
        assert!(!allowed("http://example.org/a"));
        assert!(!allowed("ftp://example.org/a"));
    }
}
