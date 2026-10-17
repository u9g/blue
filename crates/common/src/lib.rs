//! Shared by blue-daemon and the blue CLI: dialing the server and keeping a token on disk.

pub mod credentials;

use std::time::Duration;

use anyhow::{Context, bail};
use tonic::metadata::{AsciiMetadataValue, MetadataValue};
use tonic::service::Interceptor;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint, Uri};
use tonic::{Request, Status};

/// HTTP/2 keepalive ping interval for long-lived streams (ingresses drop idle connections).
pub const KEEPALIVE: Duration = Duration::from_secs(20);

/// Opens a channel to the server. TLS is required except for loopback addresses.
pub async fn connect(server_url: &str) -> anyhow::Result<Channel> {
    endpoint(server_url)?
        .connect()
        .await
        .with_context(|| format!("connect to {server_url}"))
}

/// Like [`connect`], but connects on first use instead of now.
pub fn connect_lazy(server_url: &str) -> anyhow::Result<Channel> {
    Ok(endpoint(server_url)?.connect_lazy())
}

fn endpoint(server_url: &str) -> anyhow::Result<Endpoint> {
    let uri: Uri = server_url
        .parse()
        .with_context(|| format!("invalid server URL {server_url:?}"))?;
    let mut endpoint = Endpoint::from(uri.clone())
        .connect_timeout(Duration::from_secs(10))
        .http2_keep_alive_interval(KEEPALIVE)
        .keep_alive_timeout(Duration::from_secs(20))
        .keep_alive_while_idle(true);
    match uri.scheme_str() {
        Some("https") => {
            endpoint = endpoint
                .tls_config(ClientTlsConfig::new().with_native_roots())
                .context("configure TLS")?;
        }
        Some("http") => {
            if !is_loopback(uri.host().unwrap_or_default()) {
                bail!(
                    "refusing plaintext connection to {server_url}; use https:// (plain http is \
                     only allowed for localhost)"
                );
            }
        }
        _ => bail!("server URL must start with https:// (or http:// for localhost)"),
    }
    Ok(endpoint)
}

fn is_loopback(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Adds `authorization: Bearer <token>` to every request.
#[derive(Clone)]
pub struct Bearer(Option<AsciiMetadataValue>);

impl Bearer {
    pub fn new(token: &str) -> anyhow::Result<Self> {
        let value = MetadataValue::try_from(format!("Bearer {token}"))
            .context("token contains invalid characters")?;
        Ok(Self(Some(value)))
    }

    /// No credentials, for the calls that don't need any (sign-in, pairing).
    pub fn none() -> Self {
        Self(None)
    }
}

impl Interceptor for Bearer {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        if let Some(value) = &self.0 {
            request
                .metadata_mut()
                .insert("authorization", value.clone());
        }
        Ok(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_only_for_loopback() {
        assert!(endpoint("http://localhost:50051").is_ok());
        assert!(endpoint("http://127.0.0.1:50051").is_ok());
        assert!(endpoint("http://[::1]:50051").is_ok());
        assert!(endpoint("http://server.example.com").is_err());
        assert!(endpoint("ftp://server.example.com").is_err());
    }
}
