//! Running behind a reverse proxy or ingress that terminates HTTPS.
//!
//! The server then serves plain HTTP and learns about the real client from
//! the proxy's `X-Forwarded-For` / `X-Forwarded-Proto` headers. Those
//! headers are trivial to forge, so they're honoured only when the TCP peer
//! is a trusted proxy, and never when proxy mode is off.

use std::net::IpAddr;

use anyhow::{anyhow, Result};
use axum::http::HeaderMap;
use ipnet::IpNet;

/// Trusted proxies when none are configured: loopback and private ranges,
/// which covers a proxy on the same host, a Docker network, a LAN and most
/// Kubernetes pod networks.
pub const DEFAULT_TRUSTED: &[&str] = &[
    "127.0.0.0/8",
    "::1/128",
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "fc00::/7",
];

#[derive(Clone, Debug, Default)]
pub struct ProxyTrust {
    /// Proxy mode is on.
    enabled: bool,
    trusted: Vec<IpNet>,
}

impl ProxyTrust {
    /// Proxy mode off: forwarding headers are ignored.
    pub fn off() -> Self {
        Self::default()
    }

    /// Proxy mode with the given trusted proxies (IPs or CIDRs); an empty
    /// list means [`DEFAULT_TRUSTED`].
    pub fn behind_proxy(trusted: &[String]) -> Result<Self> {
        let list: Vec<&str> = if trusted.iter().all(|s| s.trim().is_empty()) {
            DEFAULT_TRUSTED.to_vec()
        } else {
            trusted
                .iter()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect()
        };
        let trusted = list
            .into_iter()
            .map(parse_net)
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            enabled: true,
            trusted,
        })
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    fn is_trusted(&self, ip: IpAddr) -> bool {
        let ip = canonical(ip);
        self.enabled && self.trusted.iter().any(|n| n.contains(&ip))
    }

    /// The client's address: the TCP peer, or, when that peer is a trusted
    /// proxy, the right-most `X-Forwarded-For` hop that isn't one (earlier
    /// entries were written by the client and can't be trusted).
    pub fn client_ip(&self, peer: IpAddr, headers: &HeaderMap) -> IpAddr {
        if !self.is_trusted(peer) {
            return peer;
        }
        let hops: Vec<IpAddr> = headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .filter_map(|s| s.trim().parse::<IpAddr>().ok())
            .collect();
        hops.iter()
            .rev()
            .copied()
            .find(|ip| !self.is_trusted(*ip))
            // Every hop is a trusted proxy (e.g. a client on the LAN behind
            // the proxy): the left-most one is the closest to the client.
            .or_else(|| hops.first().copied())
            .unwrap_or(peer)
    }

    /// The client reached the proxy over HTTPS (`X-Forwarded-Proto: https`
    /// from a trusted proxy).
    pub fn forwarded_https(&self, peer: IpAddr, headers: &HeaderMap) -> bool {
        self.is_trusted(peer)
            && headers
                .get("x-forwarded-proto")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(',').next())
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("https"))
    }
}

/// `10.0.0.0/8`, or a bare address as a single-host network.
fn parse_net(s: &str) -> Result<IpNet> {
    s.parse::<IpNet>()
        .or_else(|_| s.parse::<IpAddr>().map(IpNet::from))
        .map_err(|_| anyhow!("trusted proxy {s:?} is not an IP address or CIDR range"))
}

/// IPv4-mapped IPv6 (`::ffff:10.0.0.1`, how a dual-stack socket reports an
/// IPv4 peer) compares as plain IPv4.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

/// Splits `OURSHELL_TRUSTED_PROXIES`-style comma/space separated lists.
pub fn split_list(s: &str) -> Vec<String> {
    s.split([',', ' ', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn off_ignores_forwarding_headers() {
        let t = ProxyTrust::off();
        let h = headers(&[
            ("x-forwarded-for", "203.0.113.7"),
            ("x-forwarded-proto", "https"),
        ]);
        assert_eq!(t.client_ip(ip("127.0.0.1"), &h), ip("127.0.0.1"));
        assert!(!t.forwarded_https(ip("127.0.0.1"), &h));
    }

    #[test]
    fn trusted_peer_reveals_the_client() {
        let t = ProxyTrust::behind_proxy(&[]).unwrap();
        let h = headers(&[
            ("x-forwarded-for", "203.0.113.7"),
            ("x-forwarded-proto", "https"),
        ]);
        assert_eq!(t.client_ip(ip("172.18.0.1"), &h), ip("203.0.113.7"));
        assert!(t.forwarded_https(ip("172.18.0.1"), &h));
        // IPv4 peer seen through a dual-stack socket.
        assert_eq!(t.client_ip(ip("::ffff:127.0.0.1"), &h), ip("203.0.113.7"));
    }

    #[test]
    fn untrusted_peer_cant_spoof() {
        let t = ProxyTrust::behind_proxy(&[]).unwrap();
        let h = headers(&[
            ("x-forwarded-for", "1.2.3.4"),
            ("x-forwarded-proto", "https"),
        ]);
        assert_eq!(t.client_ip(ip("198.51.100.9"), &h), ip("198.51.100.9"));
        assert!(!t.forwarded_https(ip("198.51.100.9"), &h));
    }

    #[test]
    fn client_supplied_hops_are_skipped() {
        // The client sent "1.2.3.4" itself; the proxy appended the real address.
        let t = ProxyTrust::behind_proxy(&[]).unwrap();
        let h = headers(&[("x-forwarded-for", "1.2.3.4, 203.0.113.7, 10.0.0.5")]);
        assert_eq!(t.client_ip(ip("10.0.0.2"), &h), ip("203.0.113.7"));
        // Several header lines count as one list.
        let h = headers(&[
            ("x-forwarded-for", "1.2.3.4"),
            ("x-forwarded-for", "203.0.113.7"),
        ]);
        assert_eq!(t.client_ip(ip("10.0.0.2"), &h), ip("203.0.113.7"));
        // A LAN client behind the proxy: every hop is private.
        let h = headers(&[("x-forwarded-for", "192.168.1.20")]);
        assert_eq!(t.client_ip(ip("127.0.0.1"), &h), ip("192.168.1.20"));
        // No header: the peer.
        assert_eq!(
            t.client_ip(ip("127.0.0.1"), &HeaderMap::new()),
            ip("127.0.0.1")
        );
    }

    #[test]
    fn custom_trusted_list() {
        let t = ProxyTrust::behind_proxy(&split_list("100.64.0.0/10, 203.0.113.1")).unwrap();
        let h = headers(&[("x-forwarded-for", "198.51.100.4")]);
        assert_eq!(t.client_ip(ip("100.64.3.2"), &h), ip("198.51.100.4"));
        assert_eq!(t.client_ip(ip("203.0.113.1"), &h), ip("198.51.100.4"));
        // Defaults no longer apply once a list is given.
        assert_eq!(t.client_ip(ip("10.0.0.1"), &h), ip("10.0.0.1"));
        assert!(ProxyTrust::behind_proxy(&["not-an-ip".into()]).is_err());
    }
}
