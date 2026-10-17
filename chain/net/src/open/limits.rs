//! Address classes and subnet grouping for connection limits.

use std::net::{IpAddr, SocketAddr};

/// Group an IP for per-subnet limits and outbound diversity: IPv4 /16, IPv6 /32
/// (IPv4-mapped IPv6 counts as IPv4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Subnet {
    V4(u16),
    V6(u32),
}

pub fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => ip,
        },
        v4 => v4,
    }
}

pub fn subnet(ip: IpAddr) -> Subnet {
    match canonical(ip) {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            Subnet::V4(u16::from_be_bytes([o[0], o[1]]))
        }
        IpAddr::V6(v6) => {
            let o = v6.octets();
            Subnet::V6(u32::from_be_bytes([o[0], o[1], o[2], o[3]]))
        }
    }
}

/// Loopback, RFC 1918, link-local, CGNAT and IPv6 unique-local addresses.
pub fn is_local(ip: IpAddr) -> bool {
    match canonical(ip) {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || (o[0] == 100 && (o[1] & 0xc0) == 64)
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            v6.is_loopback() || (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80
        }
    }
}

/// True if this IP is exempt from per-IP / per-subnet limits and IP bans. Only local
/// addresses, and only when the network runs with `allow_private` (devnets, tests).
pub fn exempt(ip: IpAddr, allow_private: bool) -> bool {
    allow_private && is_local(ip)
}

/// May this address be stored, gossiped and dialed?
pub fn dialable(a: &SocketAddr, allow_private: bool) -> bool {
    if a.port() == 0 {
        return false;
    }
    let ip = canonical(a.ip());
    let bad = match ip {
        IpAddr::V4(v4) => v4.is_unspecified() || v4.is_multicast() || v4.is_broadcast() || v4.is_documentation(),
        IpAddr::V6(v6) => v6.is_unspecified() || v6.is_multicast(),
    };
    !bad && (allow_private || !is_local(ip))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes() {
        let a: SocketAddr = "8.8.8.8:1".parse().unwrap();
        let l: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let p: SocketAddr = "10.1.2.3:1".parse().unwrap();
        assert!(dialable(&a, false));
        assert!(!dialable(&l, false) && dialable(&l, true));
        assert!(!dialable(&p, false) && dialable(&p, true));
        assert!(!dialable(&"0.0.0.0:1".parse().unwrap(), true));
        assert!(!dialable(&"8.8.8.8:0".parse().unwrap(), true));
        assert!(!dialable(&"224.0.0.1:5".parse().unwrap(), true));
        assert_eq!(subnet("1.2.3.4".parse().unwrap()), subnet("1.2.200.9".parse().unwrap()));
        assert_ne!(subnet("1.2.3.4".parse().unwrap()), subnet("1.3.3.4".parse().unwrap()));
        assert_eq!(subnet("::ffff:1.2.3.4".parse().unwrap()), subnet("1.2.9.9".parse().unwrap()));
        assert!(exempt("127.0.0.1".parse().unwrap(), true));
        assert!(!exempt("127.0.0.1".parse().unwrap(), false));
        assert!(!exempt("8.8.8.8".parse().unwrap(), true));
        assert!(is_local("fd00::1".parse().unwrap()) && is_local("100.64.0.1".parse().unwrap()));
    }
}
