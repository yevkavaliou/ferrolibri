//! Which addresses a request may connect to (D4).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Loopback, link-local, private, unspecified and other non-public addresses.
///
/// IPv6 forms that embed an IPv4 address (mapped, compatible, 6to4, NAT64) are judged by the
/// embedded address, so `::ffff:127.0.0.1` is as forbidden as `127.0.0.1`.
pub fn is_forbidden_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => forbidden_v4(v4),
        IpAddr::V6(v6) => forbidden_v6(v6),
    }
}

fn forbidden_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast()
        || a == 0 // 0.0.0.0/8, includes the unspecified address
        || (a == 100 && (64..=127).contains(&b)) // carrier-grade NAT
        || (a == 192 && b == 0 && c == 0) // IETF protocol assignments
        || (a == 192 && b == 0 && c == 2) // documentation
        || (a == 198 && (b == 18 || b == 19)) // benchmarking
        || (a == 198 && b == 51 && c == 100) // documentation
        || (a == 203 && b == 0 && c == 113) // documentation
        || a >= 240 // reserved
}

fn forbidden_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return forbidden_v4(v4);
    }
    let s = ip.segments();
    let embedded = |hi: u16, lo: u16| {
        let [a, b] = hi.to_be_bytes();
        let [c, d] = lo.to_be_bytes();
        Ipv4Addr::new(a, b, c, d)
    };
    if s[..6] == [0; 6] {
        // `::`, `::1` and the deprecated IPv4-compatible form.
        return forbidden_v4(embedded(s[6], s[7]));
    }
    if s[0] == 0x0064 && s[1] == 0xff9b && s[2..6] == [0; 4] {
        return forbidden_v4(embedded(s[6], s[7])); // NAT64
    }
    if s[0] == 0x2002 {
        return forbidden_v4(embedded(s[1], s[2])); // 6to4
    }
    ip.is_multicast()
        || (s[0] & 0xfe00) == 0xfc00 // unique local
        || (s[0] & 0xffc0) == 0xfe80 // link-local
        || (s[0] & 0xffc0) == 0xfec0 // site-local (deprecated)
        || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation
}

/// Keeps the permitted addresses. `exempt` hosts (typed by the user) keep all of them.
/// `None` when nothing is left.
pub(crate) fn vet_addresses(addresses: Vec<IpAddr>, exempt: bool) -> Option<Vec<IpAddr>> {
    let kept: Vec<IpAddr> = addresses
        .into_iter()
        .filter(|ip| exempt || !is_forbidden_ip(*ip))
        .collect();
    (!kept.is_empty()).then_some(kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forbidden(text: &str) -> bool {
        is_forbidden_ip(text.parse().unwrap())
    }

    #[test]
    fn non_public_ipv4_ranges_are_forbidden() {
        for ip in [
            "0.0.0.0",
            "0.1.2.3",
            "127.0.0.1",
            "127.255.0.9",
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "100.127.255.255",
            "192.0.0.8",
            "198.18.0.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
        ] {
            assert!(forbidden(ip), "{ip} must be forbidden");
        }
    }

    #[test]
    fn public_ipv4_addresses_are_allowed() {
        for ip in [
            "8.8.8.8",
            "1.1.1.1",
            "93.184.216.34",
            "172.15.0.1",
            "172.32.0.1",
            "100.63.0.1",
            "100.128.0.1",
            "198.20.0.1",
            "223.255.255.255",
        ] {
            assert!(!forbidden(ip), "{ip} must be allowed");
        }
    }

    #[test]
    fn non_public_ipv6_ranges_are_forbidden() {
        for ip in [
            "::",
            "::1",
            "fe80::1",
            "fc00::1",
            "fd12:3456::1",
            "fec0::1",
            "ff02::1",
            "2001:db8::1",
        ] {
            assert!(forbidden(ip), "{ip} must be forbidden");
        }
    }

    #[test]
    fn ipv6_forms_embedding_ipv4_are_judged_by_the_embedded_address() {
        for ip in [
            "::ffff:127.0.0.1",
            "::ffff:10.1.2.3",
            "::127.0.0.1",
            "64:ff9b::7f00:1",
            "64:ff9b::a00:1",
            "2002:7f00:1::1",
            "2002:c0a8:101::1",
        ] {
            assert!(forbidden(ip), "{ip} must be forbidden");
        }
        for ip in ["::ffff:8.8.8.8", "64:ff9b::808:808", "2002:808:808::1"] {
            assert!(!forbidden(ip), "{ip} must be allowed");
        }
    }

    #[test]
    fn public_ipv6_addresses_are_allowed() {
        for ip in ["2606:4700:4700::1111", "2a00:1450:4001:81b::200e"] {
            assert!(!forbidden(ip), "{ip} must be allowed");
        }
    }

    #[test]
    fn vetting_drops_forbidden_addresses_unless_exempt() {
        let mixed: Vec<IpAddr> = vec!["127.0.0.1".parse().unwrap(), "8.8.8.8".parse().unwrap()];
        assert_eq!(
            vet_addresses(mixed.clone(), false),
            Some(vec!["8.8.8.8".parse().unwrap()])
        );
        assert_eq!(vet_addresses(mixed.clone(), true), Some(mixed));
        assert_eq!(
            vet_addresses(vec!["10.0.0.1".parse().unwrap()], false),
            None
        );
        assert_eq!(vet_addresses(Vec::new(), true), None);
    }
}
