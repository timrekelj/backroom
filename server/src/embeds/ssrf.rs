//! SSRF protection: which URLs and addresses the embed fetcher may talk to.
//!
//! - Only `http` / `https`, no credentials in the URL.
//! - Ports 80, 443, 8080 and 8443 only (a restricted port set stops the
//!   fetcher from being used to probe arbitrary internal services even on
//!   public hosts). Tests with loopback allowed may use any port.
//! - Every address a host resolves to must be public (see [`is_blocked`]);
//!   one bad address rejects the host, so a mixed DNS answer cannot smuggle an
//!   internal address in. The fetcher connects to the vetted address itself
//!   (pinned, never re-resolved) and checks the connected peer again.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use url::{Host, Url};

/// Ports the fetcher may connect to (besides test loopback).
pub const ALLOWED_PORTS: [u16; 4] = [80, 443, 8080, 8443];

/// What the checks may let through beyond public addresses. Only the tests
/// ever set it (it is not configurable from the environment).
#[derive(Debug, Clone, Copy, Default)]
pub struct Policy {
    /// Allow IPv4 `127.0.0.0/8` (and any port) so tests can use a local server.
    /// Everything else non-public (RFC 1918, link-local, `::1`, …) stays blocked.
    pub allow_loopback_for_tests: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Blocked {
    Scheme,
    Credentials,
    NoHost,
    Port(u16),
    Address(IpAddr),
    Resolve(String),
}

impl std::fmt::Display for Blocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Scheme => write!(f, "scheme is not http or https"),
            Self::Credentials => write!(f, "URL contains credentials"),
            Self::NoHost => write!(f, "URL has no host"),
            Self::Port(p) => write!(f, "port {p} is not allowed"),
            Self::Address(ip) => write!(f, "address {ip} is not public"),
            Self::Resolve(e) => write!(f, "cannot resolve host: {e}"),
        }
    }
}

/// Whether `ip` is not a public unicast address: unspecified, loopback,
/// private, link-local, CGNAT, multicast, broadcast, documentation,
/// benchmarking, reserved, unique-local and the IPv6 forms that embed an IPv4
/// address (mapped, NAT64, 6to4, Teredo), which are judged by that address.
pub fn is_blocked(ip: IpAddr, policy: Policy) -> bool {
    match ip {
        IpAddr::V4(v4) => blocked_v4(v4, policy),
        IpAddr::V6(v6) => blocked_v6(v6, policy),
    }
}

fn blocked_v4(ip: Ipv4Addr, policy: Policy) -> bool {
    if policy.allow_loopback_for_tests && ip.octets()[0] == 127 {
        return false;
    }
    let [a, b, c, _] = ip.octets();
    ip.is_unspecified()
        || a == 0 // 0.0.0.0/8 "this network"
        || a == 10
        || (a == 100 && (64..=127).contains(&b)) // 100.64.0.0/10 CGNAT
        || a == 127
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 0 && c == 0) // 192.0.0.0/24 IETF protocol assignments
        || (a == 192 && b == 0 && c == 2) // TEST-NET-1
        || (a == 192 && b == 88 && c == 99) // 6to4 relay anycast
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19)) // 198.18.0.0/15 benchmarking
        || (a == 198 && b == 51 && c == 100) // TEST-NET-2
        || (a == 203 && b == 0 && c == 113) // TEST-NET-3
        || a >= 224 // multicast 224/4, reserved 240/4, broadcast
}

fn blocked_v6(ip: Ipv6Addr, policy: Policy) -> bool {
    let s = ip.segments();
    // Forms that carry an IPv4 address are judged by it.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return blocked_v4(v4, policy);
    }
    if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        // NAT64 (64:ff9b::/96)
        return blocked_v4(embedded_v4(s[6], s[7]), policy);
    }
    if s[0] == 0x2002 {
        // 6to4: the next 32 bits are the IPv4 address
        return blocked_v4(embedded_v4(s[1], s[2]), policy);
    }
    ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast() // ff00::/8
        || (s[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
        || (s[0] & 0xffc0) == 0xfec0 // fec0::/10 deprecated site-local
        || (s[0] & 0xfe00) == 0xfc00 // fc00::/7 unique local
        || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation 2001:db8::/32
        || (s[0] == 0x2001 && s[1] == 0) // Teredo 2001::/32
        || (s[0] == 0x2001 && (s[1] & 0xfff0) == 0x0010) // ORCHID 2001:10::/28
        || (s[0] == 0x2001 && (s[1] & 0xfff0) == 0x0020) // ORCHIDv2 2001:20::/28
        || (s[0] == 0x0100 && s[1] == 0 && s[2] == 0 && s[3] == 0) // discard-only 100::/64
        || (s[0] == 0x0064 && s[1] == 0xff9b && s[2] == 1) // local-use NAT64 64:ff9b:1::/48
        || s[..6] == [0, 0, 0, 0, 0, 0] // IPv4-compatible ::/96 (deprecated)
        || (s[0] == 0x3fff && (s[1] & 0xf000) == 0) // documentation 3fff::/20
}

fn embedded_v4(hi: u16, lo: u16) -> Ipv4Addr {
    let (a, b) = (hi.to_be_bytes(), lo.to_be_bytes());
    Ipv4Addr::new(a[0], a[1], b[0], b[1])
}

/// Checks everything about `url` that needs no DNS: scheme, credentials,
/// host, port. Returns the host and port to connect to.
pub fn check_url(url: &Url, policy: Policy) -> Result<(Host<String>, u16), Blocked> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Blocked::Scheme);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Blocked::Credentials);
    }
    let host = url.host().ok_or(Blocked::NoHost)?.to_owned();
    let port = url.port_or_known_default().ok_or(Blocked::NoHost)?;
    if !ALLOWED_PORTS.contains(&port) && !policy.allow_loopback_for_tests {
        return Err(Blocked::Port(port));
    }
    Ok((host, port))
}

/// Resolves the host of `url` (IP literals are taken as they are) and
/// vets every address. Returns the address to connect to.
pub async fn vet(url: &Url, policy: Policy) -> Result<SocketAddr, Blocked> {
    let (host, port) = check_url(url, policy)?;
    let addrs: Vec<IpAddr> = match host {
        Host::Ipv4(ip) => vec![IpAddr::V4(ip)],
        Host::Ipv6(ip) => vec![IpAddr::V6(ip)],
        Host::Domain(name) => tokio::net::lookup_host((name.as_str(), port))
            .await
            .map_err(|e| Blocked::Resolve(e.to_string()))?
            .map(|a| a.ip())
            .collect(),
    };
    pick(&addrs, policy).map(|ip| SocketAddr::new(ip, port))
}

/// The first of `addrs`, provided none of them is blocked.
fn pick(addrs: &[IpAddr], policy: Policy) -> Result<IpAddr, Blocked> {
    if let Some(bad) = addrs.iter().find(|ip| is_blocked(**ip, policy)) {
        return Err(Blocked::Address(*bad));
    }
    addrs.first().copied().ok_or_else(|| Blocked::Resolve("no addresses".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STRICT: Policy = Policy { allow_loopback_for_tests: false };
    const LOOPBACK: Policy = Policy { allow_loopback_for_tests: true };

    fn blocked(s: &str, policy: Policy) -> bool {
        is_blocked(s.parse().unwrap(), policy)
    }

    #[test]
    fn ipv4_non_public_ranges_are_blocked() {
        for ip in [
            "0.0.0.0", "0.1.2.3", "10.0.0.1", "10.255.255.255", "100.64.0.1", "100.127.255.255", "127.0.0.1", "127.255.255.254",
            "169.254.169.254", "169.254.0.1", "172.16.0.1", "172.31.255.255", "192.0.0.1", "192.0.2.5", "192.88.99.1",
            "192.168.0.1", "192.168.255.255", "198.18.0.1", "198.19.255.255", "198.51.100.7", "203.0.113.9", "224.0.0.1",
            "239.255.255.250", "240.0.0.1", "255.255.255.255",
        ] {
            assert!(blocked(ip, STRICT), "{ip}");
        }
    }

    #[test]
    fn ipv4_public_addresses_and_range_edges_are_allowed() {
        for ip in [
            "1.1.1.1", "8.8.8.8", "9.255.255.255", "11.0.0.1", "100.63.255.255", "100.128.0.1", "126.255.255.255", "128.0.0.1",
            "169.253.255.255", "169.255.0.1", "172.15.255.255", "172.32.0.1", "192.0.1.1", "192.0.3.1", "192.88.98.1",
            "192.167.255.255", "192.169.0.1", "198.17.255.255", "198.20.0.1", "198.51.99.1", "198.51.101.1", "203.0.112.1",
            "203.0.114.1", "223.255.255.255", "93.184.216.34",
        ] {
            assert!(!blocked(ip, STRICT), "{ip}");
        }
    }

    #[test]
    fn ipv6_non_public_ranges_are_blocked() {
        for ip in [
            "::", "::1", "fe80::1", "febf::1", "fec0::1", "fc00::1", "fd12:3456::1", "fdff::1", "ff02::1", "ff00::", "2001:db8::1",
            "2001:0::1", "2001:10::1", "2001:2f::1", "100::1", "64:ff9b:1::1", "::2", "::ffff:0.0.0.0", "3fff::1",
            // IPv4-mapped / NAT64 / 6to4 judged by the embedded address
            "::ffff:10.0.0.1", "::ffff:127.0.0.1", "::ffff:169.254.169.254", "::ffff:192.168.1.1", "64:ff9b::a00:1",
            "64:ff9b::7f00:1", "2002:a00:1::1", "2002:7f00:1::", "2002:c0a8:101::1", "2002:a9fe:a9fe::1",
        ] {
            assert!(blocked(ip, STRICT), "{ip}");
        }
    }

    #[test]
    fn ipv6_public_addresses_are_allowed() {
        for ip in [
            "2606:4700:4700::1111", "2001:4860:4860::8888", "2a00:1450:4001:81b::200e", "2001:1::1", "2001:200::1", "::ffff:8.8.8.8",
            "64:ff9b::808:808", "2002:808:808::1", "fe7f::1", "fbff::1", "2000::1",
        ] {
            assert!(!blocked(ip, STRICT), "{ip}");
        }
    }

    #[test]
    fn loopback_policy_exempts_only_ipv4_loopback() {
        assert!(!blocked("127.0.0.1", LOOPBACK));
        assert!(!blocked("127.5.5.5", LOOPBACK));
        assert!(!blocked("::ffff:127.0.0.1", LOOPBACK));
        for ip in [
            "::1", "10.0.0.1", "172.16.0.1", "192.168.1.1", "169.254.169.254", "100.64.0.1", "0.0.0.0", "224.0.0.1", "fe80::1",
            "fc00::1", "::ffff:10.0.0.1", "::",
        ] {
            assert!(blocked(ip, LOOPBACK), "{ip}");
        }
    }

    #[test]
    fn one_bad_address_rejects_the_whole_answer() {
        let ok = "93.184.216.34".parse().unwrap();
        let bad: IpAddr = "10.0.0.1".parse().unwrap();
        assert_eq!(pick(&[ok], STRICT), Ok(ok));
        assert_eq!(pick(&[ok, bad], STRICT), Err(Blocked::Address(bad)));
        assert_eq!(pick(&[bad, ok], STRICT), Err(Blocked::Address(bad)));
        assert!(matches!(pick(&[], STRICT), Err(Blocked::Resolve(_))));
    }

    fn check(url: &str, policy: Policy) -> Result<u16, Blocked> {
        check_url(&Url::parse(url).unwrap(), policy).map(|(_, port)| port)
    }

    #[test]
    fn urls_are_screened_for_scheme_credentials_and_port() {
        assert_eq!(check("http://example.com/", STRICT), Ok(80));
        assert_eq!(check("https://example.com/", STRICT), Ok(443));
        assert_eq!(check("http://example.com:8080/", STRICT), Ok(8080));
        assert_eq!(check("https://example.com:8443/", STRICT), Ok(8443));
        assert_eq!(check("ftp://example.com/", STRICT), Err(Blocked::Scheme));
        assert_eq!(check("file:///etc/passwd", STRICT), Err(Blocked::Scheme));
        assert_eq!(check("gopher://example.com/", STRICT), Err(Blocked::Scheme));
        assert_eq!(check("https://u:p@example.com/", STRICT), Err(Blocked::Credentials));
        assert_eq!(check("https://u@example.com/", STRICT), Err(Blocked::Credentials));
        assert_eq!(check("http://example.com:22/", STRICT), Err(Blocked::Port(22)));
        assert_eq!(check("https://example.com:6379/", STRICT), Err(Blocked::Port(6379)));
        // Only the test policy opens other ports.
        assert_eq!(check("http://127.0.0.1:41234/", LOOPBACK), Ok(41234));
    }

    #[tokio::test]
    async fn ip_literals_are_vetted_without_dns() {
        let vet = |u: &'static str, p| async move { vet(&Url::parse(u).unwrap(), p).await };
        assert!(matches!(vet("http://127.0.0.1/", STRICT).await, Err(Blocked::Address(_))));
        assert!(matches!(vet("http://10.1.2.3/", STRICT).await, Err(Blocked::Address(_))));
        assert!(matches!(vet("http://169.254.169.254/latest/meta-data", STRICT).await, Err(Blocked::Address(_))));
        assert!(matches!(vet("http://[::1]/", STRICT).await, Err(Blocked::Address(_))));
        assert!(matches!(vet("http://[::1]:8080/", LOOPBACK).await, Err(Blocked::Address(_))));
        assert!(matches!(vet("http://[::ffff:10.0.0.1]/", STRICT).await, Err(Blocked::Address(_))));
        assert!(matches!(vet("http://10.0.0.1:9999/", LOOPBACK).await, Err(Blocked::Address(_))));
        // Alternative spellings of 127.0.0.1 are normalised by the URL parser.
        for u in ["http://2130706433/", "http://0x7f000001/", "http://0177.0.0.1/", "http://127.1/", "http://0/"] {
            assert!(matches!(vet(u, STRICT).await, Err(Blocked::Address(_))), "{u}");
        }
        assert_eq!(vet("http://127.0.0.1:5000/", LOOPBACK).await, Ok("127.0.0.1:5000".parse().unwrap()));
        assert_eq!(vet("https://93.184.216.34/", STRICT).await, Ok("93.184.216.34:443".parse().unwrap()));
    }

    #[tokio::test]
    async fn names_resolving_to_loopback_are_blocked() {
        let u = Url::parse("http://localhost/").unwrap();
        assert!(matches!(vet(&u, STRICT).await, Err(Blocked::Address(_) | Blocked::Resolve(_))));
    }
}
