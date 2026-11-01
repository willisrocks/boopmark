//! Outbound URL safety shared by every adapter that fetches a user-supplied
//! URL or forwards it to a third party.

use crate::domain::error::DomainError;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use url::Url;

/// Reject URLs that should not be forwarded to third-party metadata APIs.
/// Only allows public http/https URLs; strips credentials and fragments.
pub fn validate_public_url(url: &str) -> Result<String, DomainError> {
    let parsed =
        Url::parse(url).map_err(|e| DomainError::InvalidInput(format!("invalid URL: {e}")))?;
    UrlGuard::public()
        .check(&parsed)
        .map_err(|e| DomainError::InvalidInput(e.to_string()))?;

    // Return URL without fragment
    let mut clean = parsed;
    clean.set_fragment(None);
    Ok(clean.to_string())
}

/// True for addresses on the public internet. Private, loopback, link-local,
/// shared (CGNAT), documentation, benchmarking, multicast, broadcast,
/// unspecified and reserved ranges are all refused, and IPv4-mapped or
/// IPv4-compatible IPv6 addresses are judged by their IPv4 address.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_ipv4(ip),
        IpAddr::V6(ip) => match ip.to_ipv4() {
            Some(v4) if !ip.is_loopback() && !ip.is_unspecified() => is_public_ipv4(v4),
            _ => is_public_ipv6(ip),
        },
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.is_multicast()
        || a == 0 // "this network" 0.0.0.0/8
        || (a == 100 && (b & 0xc0) == 64) // shared address space 100.64.0.0/10
        || (a == 192 && b == 0 && ip.octets()[2] == 0) // IETF protocol assignments 192.0.0.0/24
        || (a == 198 && (b & 0xfe) == 18) // benchmarking 198.18.0.0/15
        || a >= 240) // reserved 240.0.0.0/4
}

fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (first & 0xfe00) == 0xfc00 // unique-local fc00::/7
        || (first & 0xffc0) == 0xfe80 // link-local fe80::/10
        || (first & 0xffc0) == 0xfec0 // deprecated site-local fec0::/10
        || (first == 0x2001 && ip.segments()[1] == 0x0db8)) // documentation 2001:db8::/32
}

/// A URL or an address that the guard refuses. Adapters find it in an
/// error's source chain to tell a refusal from a network failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Blocked(pub String);

impl Blocked {
    /// Whether `err` or any of its sources is a `Blocked` refusal.
    pub fn in_chain<'a>(err: &'a (dyn std::error::Error + 'static)) -> Option<&'a Blocked> {
        let mut current = Some(err);
        while let Some(e) = current {
            if let Some(blocked) = e.downcast_ref::<Blocked>() {
                return Some(blocked);
            }
            current = e.source();
        }
        None
    }
}

/// The rules for which URLs and addresses may be fetched.
#[derive(Clone, Copy)]
pub struct UrlGuard {
    allow_ip: fn(IpAddr) -> bool,
}

impl UrlGuard {
    pub fn public() -> Self {
        Self {
            allow_ip: is_public_ip,
        }
    }

    /// Also allows loopback, so tests can fetch from a local fixture server.
    #[cfg(test)]
    pub fn allowing_loopback() -> Self {
        Self {
            allow_ip: |ip| ip.is_loopback() || is_public_ip(ip),
        }
    }

    pub fn allows_ip(&self, ip: IpAddr) -> bool {
        (self.allow_ip)(ip)
    }

    /// Check the parts of a URL that need no DNS lookup: scheme,
    /// credentials, local host names and literal IP addresses. Host names
    /// are checked again after resolution by `PublicOnlyResolver`.
    pub fn check(&self, url: &Url) -> Result<(), Blocked> {
        match url.scheme() {
            "http" | "https" => {}
            other => return Err(Blocked(format!("unsupported scheme: {other}"))),
        }

        if !url.username().is_empty() || url.password().is_some() {
            return Err(Blocked(
                "URL contains credentials; refusing to forward to third-party API".to_string(),
            ));
        }

        if let Some(host) = url.host_str() {
            let normalized = host.trim_matches(['[', ']']).to_ascii_lowercase();
            let local_name = normalized == "localhost"
                || normalized.ends_with(".localhost")
                || normalized.ends_with(".local")
                || normalized.ends_with(".internal");
            let local_ip = normalized
                .parse::<IpAddr>()
                .is_ok_and(|ip| !self.allows_ip(ip));
            if local_name || local_ip {
                return Err(Blocked(
                    "private/local URL; refusing to fetch or forward it".to_string(),
                ));
            }
        }
        Ok(())
    }
}

/// Resolves a host name to socket addresses.
pub trait Lookup: Send + Sync + 'static {
    fn lookup(
        &self,
        host: String,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send>>;
}

/// The operating system's resolver.
pub struct SystemLookup;

impl Lookup for SystemLookup {
    fn lookup(
        &self,
        host: String,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send>> {
        Box::pin(async move { Ok(tokio::net::lookup_host((host, 0)).await?.collect()) })
    }
}

/// A reqwest DNS resolver that fails the lookup when any address of the
/// name is refused by the guard. The connection uses exactly the addresses
/// checked here, so a name cannot be rebound to a private address between
/// the check and the connect, and every redirect hop goes through it too.
pub struct PublicOnlyResolver<L> {
    lookup: Arc<L>,
    guard: UrlGuard,
}

impl<L: Lookup> PublicOnlyResolver<L> {
    pub fn new(lookup: L, guard: UrlGuard) -> Self {
        Self {
            lookup: Arc::new(lookup),
            guard,
        }
    }

    pub async fn resolve_host(
        lookup: &L,
        guard: UrlGuard,
        host: String,
    ) -> Result<Vec<SocketAddr>, Box<dyn std::error::Error + Send + Sync>> {
        let addrs = lookup.lookup(host.clone()).await?;
        if addrs.is_empty() {
            return Err(format!("{host} did not resolve").into());
        }
        if let Some(addr) = addrs.iter().find(|a| !guard.allows_ip(a.ip())) {
            return Err(Box::new(Blocked(format!(
                "{host} resolves to a private address ({}); refusing to fetch it",
                addr.ip()
            ))));
        }
        Ok(addrs)
    }
}

impl<L: Lookup> reqwest::dns::Resolve for PublicOnlyResolver<L> {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let lookup = self.lookup.clone();
        let guard = self.guard;
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs = Self::resolve_host(&lookup, guard, host).await?;
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// A redirect policy that re-checks every hop with the guard and stops after
/// `max_hops` redirects.
pub fn guarded_redirect_policy(guard: UrlGuard, max_hops: usize) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() > max_hops {
            return attempt.error(format!("more than {max_hops} redirects"));
        }
        match guard.check(attempt.url()) {
            Ok(()) => attempt.follow(),
            Err(blocked) => attempt.error(blocked),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn accepts_public_https_url() {
        let result = validate_public_url("https://medium.com/article");
        assert!(result.is_ok());
    }

    #[test]
    fn accepts_public_http_url() {
        let result = validate_public_url("http://example.com/page");
        assert!(result.is_ok());
    }

    #[test]
    fn rejects_ftp_scheme() {
        let result = validate_public_url("ftp://files.example.com/doc");
        assert!(result.is_err());
    }

    #[test]
    fn rejects_url_with_credentials() {
        let result = validate_public_url("https://user:pass@example.com/page");
        assert!(result.is_err());
    }

    #[test]
    fn rejects_localhost() {
        let result = validate_public_url("http://localhost:3000/api");
        assert!(result.is_err());
    }

    #[test]
    fn rejects_loopback_ip() {
        let result = validate_public_url("http://127.0.0.1:8080/path");
        assert!(result.is_err());
    }

    #[test]
    fn rejects_private_ip_10() {
        let result = validate_public_url("http://10.0.0.1/internal");
        assert!(result.is_err());
    }

    #[test]
    fn rejects_private_ip_192_168() {
        let result = validate_public_url("http://192.168.1.1/admin");
        assert!(result.is_err());
    }

    #[test]
    fn rejects_private_ip_across_the_full_172_16_range() {
        assert!(validate_public_url("http://172.31.255.254/internal").is_err());
    }

    #[test]
    fn rejects_ipv6_unique_local_and_link_local_addresses() {
        assert!(validate_public_url("http://[fc00::1]/internal").is_err());
        assert!(validate_public_url("http://[fe80::1]/internal").is_err());
    }

    #[test]
    fn strips_fragment_from_url() {
        let result = validate_public_url("https://example.com/page#section").unwrap();
        assert_eq!(result, "https://example.com/page");
    }

    #[test]
    fn rejects_invalid_url() {
        let result = validate_public_url("not a url");
        assert!(result.is_err());
    }

    #[test]
    fn rejects_metadata_and_mapped_addresses() {
        for url in [
            "http://169.254.169.254/latest/meta-data/",
            "http://[::ffff:127.0.0.1]/",
            "http://[::ffff:10.0.0.1]/",
            "http://100.64.0.1/",
            "http://0.0.0.0/",
            "http://[::]/",
            "http://[::1]/",
            "file:///etc/passwd",
            "gopher://example.com/",
        ] {
            assert!(validate_public_url(url).is_err(), "{url} must be refused");
        }
    }

    #[test]
    fn public_ip_ranges() {
        for ip in [
            "8.8.8.8",
            "1.1.1.1",
            "2606:4700:4700::1111",
            "::ffff:8.8.8.8",
        ] {
            assert!(is_public_ip(ip.parse().unwrap()), "{ip} is public");
        }
        for ip in [
            "10.1.2.3",
            "172.16.0.1",
            "192.168.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "100.127.255.255",
            "192.0.0.8",
            "198.19.0.1",
            "224.0.0.1",
            "255.255.255.255",
            "240.0.0.1",
            "fd12::1",
            "fe80::1",
            "fec0::1",
            "2001:db8::1",
            "ff02::1",
        ] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip} is not public");
        }
    }

    struct FakeLookup(HashMap<&'static str, Vec<&'static str>>);

    impl Lookup for FakeLookup {
        fn lookup(
            &self,
            host: String,
        ) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send>> {
            let addrs = self
                .0
                .get(host.as_str())
                .map(|ips| {
                    ips.iter()
                        .map(|ip| SocketAddr::new(ip.parse().unwrap(), 0))
                        .collect()
                })
                .unwrap_or_default();
            Box::pin(async move { Ok(addrs) })
        }
    }

    async fn resolve(lookup: &FakeLookup, host: &str) -> Result<Vec<SocketAddr>, String> {
        PublicOnlyResolver::resolve_host(lookup, UrlGuard::public(), host.to_string())
            .await
            .map_err(|e| e.to_string())
    }

    #[tokio::test]
    async fn resolver_refuses_names_that_resolve_to_private_addresses() {
        let lookup = FakeLookup(HashMap::from([
            ("public.example", vec!["93.184.216.34"]),
            ("intranet.example", vec!["10.0.0.5"]),
            ("mixed.example", vec!["93.184.216.34", "192.168.1.10"]),
            ("mapped.example", vec!["::ffff:127.0.0.1"]),
        ]));

        assert_eq!(resolve(&lookup, "public.example").await.unwrap().len(), 1);
        let err = resolve(&lookup, "intranet.example").await.unwrap_err();
        assert!(err.contains("private address (10.0.0.5)"), "{err}");
        // One private answer poisons the whole name.
        assert!(resolve(&lookup, "mixed.example").await.is_err());
        assert!(resolve(&lookup, "mapped.example").await.is_err());
        assert!(resolve(&lookup, "nothing.example").await.is_err());
    }

    #[tokio::test]
    async fn resolver_error_is_recognizable_as_blocked() {
        let lookup = FakeLookup(HashMap::from([("intranet.example", vec!["10.0.0.5"])]));
        let err = PublicOnlyResolver::resolve_host(
            &lookup,
            UrlGuard::public(),
            "intranet.example".into(),
        )
        .await
        .unwrap_err();
        assert!(Blocked::in_chain(err.as_ref()).is_some());
    }
}
