//! IP address validation for SSRF protection.

use super::Host;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Check if a host is a loopback hostname or IP.
pub fn is_loopback_host(host: &Host) -> bool {
    let host_str = host.as_str();

    // Strip zone ID if present (e.g., "fe80::1%eth0")
    let host_str = host_str
        .split_once('%')
        .map(|(ip, _)| ip)
        .unwrap_or(host_str);

    // Check common loopback hostnames
    if host_str == "localhost" || host_str == "localhost.localdomain" {
        return true;
    }

    // Try to parse as IP address
    if let Ok(ip) = host_str.parse::<IpAddr>() {
        return ip.is_loopback();
    }

    false
}

/// Check if an IP address is non-public (private, local, reserved, etc.).
/// This provides comprehensive SSRF protection.
pub fn is_non_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_non_public_ipv4(ip),
        IpAddr::V6(ip) => is_non_public_ipv6(ip),
    }
}

/// Check if an IPv4 address is non-public.
/// Covers all RFC-defined non-public ranges.
pub fn is_non_public_ipv4(ip: Ipv4Addr) -> bool {
    ip.is_loopback()                              // 127.0.0.0/8 (RFC 1122)
        || ip.is_private()                        // 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16 (RFC 1918)
        || ip.is_link_local()                     // 169.254.0.0/16 (RFC 3927)
        || ip.is_unspecified()                    // 0.0.0.0
        || ip.is_multicast()                      // 224.0.0.0/4 (RFC 3171)
        || ip.is_broadcast()                      // 255.255.255.255
        || ipv4_in_cidr(ip, [0, 0, 0, 0], 8)      // "this network" (RFC 1122)
        || ipv4_in_cidr(ip, [100, 64, 0, 0], 10)  // CGNAT / Shared Address Space (RFC 6598)
        || ipv4_in_cidr(ip, [192, 0, 0, 0], 24)   // IETF Protocol Assignments (RFC 6890)
        || ipv4_in_cidr(ip, [192, 0, 2, 0], 24)   // TEST-NET-1 / Documentation (RFC 5737)
        || ipv4_in_cidr(ip, [198, 18, 0, 0], 15)  // Benchmarking (RFC 2544)
        || ipv4_in_cidr(ip, [198, 51, 100, 0], 24) // TEST-NET-2 / Documentation (RFC 5737)
        || ipv4_in_cidr(ip, [203, 0, 113, 0], 24) // TEST-NET-3 / Documentation (RFC 5737)
        || ipv4_in_cidr(ip, [240, 0, 0, 0], 4) // Reserved for Future Use (RFC 6890)
}

/// Check if an IPv6 address is non-public.
pub fn is_non_public_ipv6(ip: Ipv6Addr) -> bool {
    // Check if it's an IPv4-mapped IPv6 address
    if let Some(v4) = ip.to_ipv4() {
        return is_non_public_ipv4(v4) || ip.is_loopback();
    }

    ip.is_loopback()                              // ::1
        || ip.is_unspecified()                    // ::
        || ip.is_multicast()                      // ff00::/8
        || is_unique_local_ipv6(&ip)              // fc00::/7 (RFC 4193)
        || is_link_local_ipv6(&ip)                // fe80::/10
        || is_documentation_ipv6(&ip)             // 2001:db8::/32 (RFC 3849)
        || is_discard_ipv6(&ip) // 100::/64 (RFC 6666)
}

/// Check if IPv4 is in a CIDR range.
fn ipv4_in_cidr(ip: Ipv4Addr, base: [u8; 4], prefix: u8) -> bool {
    let ip = u32::from(ip);
    let base = u32::from(Ipv4Addr::from(base));
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    (ip & mask) == (base & mask)
}

/// Check if IPv6 is unique local (fc00::/7).
fn is_unique_local_ipv6(ip: &Ipv6Addr) -> bool {
    let octets = ip.octets();
    (octets[0] & 0xfe) == 0xfc
}

/// Check if IPv6 is link-local (fe80::/10).
fn is_link_local_ipv6(ip: &Ipv6Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 0xfe && (octets[1] & 0xc0) == 0x80
}

/// Check if IPv6 is documentation (2001:db8::/32).
fn is_documentation_ipv6(ip: &Ipv6Addr) -> bool {
    let segments = ip.segments();
    segments[0] == 0x2001 && segments[1] == 0x0db8
}

/// Check if IPv6 is discard prefix (100::/64).
fn is_discard_ipv6(ip: &Ipv6Addr) -> bool {
    let segments = ip.segments();
    segments[0] == 0x0100 && segments[1] == 0 && segments[2] == 0 && segments[3] == 0
}

/// Resolve a hostname and check if any of its IPs are non-public.
/// This provides DNS rebinding protection.
pub async fn host_resolves_to_non_public(host: &str, _port: u16) -> bool {
    // Use tokio's DNS resolution
    let addr = format!("{}:0", host);
    match tokio::net::lookup_host(&addr).await {
        Ok(addrs) => {
            for addr in addrs {
                if is_non_public_ip(addr.ip()) {
                    return true;
                }
            }
            false
        }
        Err(_) => {
            // If we can't resolve, be safe and allow (the connection will fail anyway)
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_loopback_host() {
        assert!(is_loopback_host(&Host::parse("localhost").unwrap()));
        assert!(is_loopback_host(&Host::parse("127.0.0.1").unwrap()));
        assert!(is_loopback_host(&Host::parse("::1").unwrap()));
        assert!(!is_loopback_host(&Host::parse("example.com").unwrap()));
    }

    #[test]
    fn test_is_non_public_ipv4() {
        // Loopback
        assert!(is_non_public_ipv4("127.0.0.1".parse().unwrap()));
        assert!(is_non_public_ipv4("127.255.255.255".parse().unwrap()));

        // Private ranges
        assert!(is_non_public_ipv4("10.0.0.1".parse().unwrap()));
        assert!(is_non_public_ipv4("172.16.0.1".parse().unwrap()));
        assert!(is_non_public_ipv4("192.168.1.1".parse().unwrap()));

        // Link-local
        assert!(is_non_public_ipv4("169.254.1.1".parse().unwrap()));

        // CGNAT
        assert!(is_non_public_ipv4("100.64.0.1".parse().unwrap()));

        // Test networks
        assert!(is_non_public_ipv4("192.0.2.1".parse().unwrap()));
        assert!(is_non_public_ipv4("198.51.100.1".parse().unwrap()));
        assert!(is_non_public_ipv4("203.0.113.1".parse().unwrap()));

        // Public IPs should be allowed
        assert!(!is_non_public_ipv4("8.8.8.8".parse().unwrap()));
        assert!(!is_non_public_ipv4("1.1.1.1".parse().unwrap()));
    }

    #[test]
    fn test_is_non_public_ipv6() {
        assert!(is_non_public_ipv6("::1".parse().unwrap()));
        assert!(is_non_public_ipv6("fc00::1".parse().unwrap()));
        assert!(is_non_public_ipv6("fe80::1".parse().unwrap()));
        assert!(is_non_public_ipv6("2001:db8::1".parse().unwrap()));

        // Public IPv6 should be allowed
        assert!(!is_non_public_ipv6("2606:4700::1".parse().unwrap()));
    }
}
