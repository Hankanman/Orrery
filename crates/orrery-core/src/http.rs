//! Shared HTTP client setup.
//!
//! reqwest is built with `rustls-no-provider` so the binary doesn't carry
//! aws-lc-rs (the provider reqwest 0.13 bundles by default). That makes the
//! rustls crypto provider ours to install, before any `Client` is built, or
//! reqwest panics. Every client in this crate starts from [`client_builder`].

/// A `reqwest::ClientBuilder` with the process-wide rustls provider (`ring`)
/// installed. Cheap to call repeatedly: the install only happens once.
pub fn client_builder() -> reqwest::ClientBuilder {
    install_crypto_provider();
    reqwest::Client::builder()
}

/// Install `ring` as the process-default rustls crypto provider, if no provider
/// is installed yet. Idempotent.
pub fn install_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // Err means another provider was installed first, which is fine: reqwest
        // uses whatever the process default is.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn builds_a_client() {
        assert!(super::client_builder().build().is_ok());
        // A second call must not panic or re-install.
        assert!(super::client_builder().build().is_ok());
    }
}
