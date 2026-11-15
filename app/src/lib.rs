//! Telinhas desktop client.

/// Human-readable name and version, used by the CLI and later by the UI.
pub fn banner() -> String {
    format!("Telinhas {}", env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_contains_name_and_version() {
        let banner = banner();
        assert!(banner.starts_with("Telinhas "));
        assert!(banner.ends_with(env!("CARGO_PKG_VERSION")));
    }
}
