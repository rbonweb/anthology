//! `pycompat::ipaddress`: what `ipaddress.IPv4Address(s)` and `ipaddress.IPv6Address(s)` of
//! CPython 3.12 accept (design §12), and the network checks privacy uses.

/// `ipaddress.IPv4Address(s)`'s octets, or `None` where it raises: exactly four `.`-separated
/// parts, each 1–3 ASCII digits, no leading zero (but `0` itself), at most 255; no `/`.
pub fn parse_v4(s: &str) -> Option<[u8; 4]> {
    if s.is_empty() || s.contains('/') {
        return None;
    }
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut out = [0u8; 4];
    for (o, part) in out.iter_mut().zip(parts) {
        *o = parse_octet(part)?;
    }
    Some(out)
}

fn parse_octet(s: &str) -> Option<u8> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) || s.len() > 3 {
        return None;
    }
    if s != "0" && s.starts_with('0') {
        return None;
    }
    s.parse::<u16>().ok().and_then(|n| u8::try_from(n).ok())
}

/// `ipaddress.IPv6Address(s)`'s eight groups, or `None` where it raises. A `%scope` suffix
/// is accepted and dropped as Python 3.9+ does (rejected when empty or holding another `%`);
/// privacy strips it before calling, so either way works there.
pub fn parse_v6(s: &str) -> Option<[u16; 8]> {
    if s.contains('/') {
        return None;
    }
    let addr = match s.split_once('%') {
        None => s,
        Some((a, scope)) => {
            if scope.is_empty() || scope.contains('%') {
                return None;
            }
            a
        }
    };
    v6_groups(addr)
}

/// `IPv6Address._ip_int_from_string`.
fn v6_groups(ip: &str) -> Option<[u16; 8]> {
    const HEXTETS: usize = 8;
    if ip.is_empty() || ip.chars().count() > 45 {
        return None;
    }
    let max_parts = HEXTETS + 1;
    // ip.split(':', maxsplit=max_parts)
    let mut parts: Vec<String> = ip.splitn(max_parts + 1, ':').map(str::to_string).collect();
    if parts.len() < 3 {
        return None;
    }
    if parts.last().is_some_and(|p| p.contains('.')) {
        let v4 = parse_v4(&parts.pop().unwrap_or_default())?;
        parts.push(format!("{:x}", u16::from_be_bytes([v4[0], v4[1]])));
        parts.push(format!("{:x}", u16::from_be_bytes([v4[2], v4[3]])));
    }
    if parts.len() > max_parts {
        return None;
    }
    let mut skip_index = None;
    for (i, p) in parts.iter().enumerate().take(parts.len() - 1).skip(1) {
        if p.is_empty() {
            if skip_index.is_some() {
                return None;
            }
            skip_index = Some(i);
        }
    }
    let (parts_hi, parts_lo, skipped) = match skip_index {
        Some(k) => {
            let mut hi = k;
            let mut lo = parts.len() - k - 1;
            if parts[0].is_empty() {
                hi -= 1;
                if hi > 0 {
                    return None;
                }
            }
            if parts[parts.len() - 1].is_empty() {
                lo -= 1;
                if lo > 0 {
                    return None;
                }
            }
            let skipped = HEXTETS.checked_sub(hi + lo).filter(|&n| n >= 1)?;
            (hi, lo, skipped)
        }
        None => {
            if parts.len() != HEXTETS || parts[0].is_empty() || parts[HEXTETS - 1].is_empty() {
                return None;
            }
            (parts.len(), 0, 0)
        }
    };
    let mut out = [0u16; 8];
    let mut at = 0;
    for p in &parts[..parts_hi] {
        out[at] = parse_hextet(p)?;
        at += 1;
    }
    at += skipped;
    for p in &parts[parts.len() - parts_lo..] {
        out[at] = parse_hextet(p)?;
        at += 1;
    }
    Some(out)
}

fn parse_hextet(s: &str) -> Option<u16> {
    if !s.bytes().all(|b| b.is_ascii_hexdigit()) || s.len() > 4 {
        return None;
    }
    // `int("", 16)` raises.
    u16::from_str_radix(s, 16).ok()
}

/// `ip_address(addr) in ip_network(net/prefix)` for IPv4 (`prefix` 0..=32).
pub fn v4_in(addr: [u8; 4], net: [u8; 4], prefix: u8) -> bool {
    let mask = u32::MAX
        .checked_shl(32 - u32::from(prefix.min(32)))
        .unwrap_or(0);
    u32::from_be_bytes(addr) & mask == u32::from_be_bytes(net) & mask
}

/// `ip_address(addr) in ip_network(net/prefix)` for IPv6 (`prefix` 0..=128).
pub fn v6_in(addr: &[u16; 8], net: &[u16; 8], prefix: u8) -> bool {
    let to = |g: &[u16; 8]| g.iter().fold(0u128, |acc, &x| (acc << 16) | u128::from(x));
    let mask = u128::MAX
        .checked_shl(128 - u32::from(prefix.min(128)))
        .unwrap_or(0);
    to(addr) & mask == to(net) & mask
}

/// `addr in ipaddress.ip_network("fc00::/7")` (unique local).
pub fn is_unique_local_v6(g: &[u16; 8]) -> bool {
    g[0] & 0xfe00 == 0xfc00
}

/// `addr in ipaddress.ip_network("fe80::/10")` (link local).
pub fn is_link_local_v6(g: &[u16; 8]) -> bool {
    g[0] & 0xffc0 == 0xfe80
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v4_is_strict() {
        assert_eq!(parse_v4("10.0.0.1"), Some([10, 0, 0, 1]));
        for bad in [
            "",
            "1.2.3",
            "1.2.3.4.5",
            "01.2.3.4",
            "256.1.1.1",
            "1.2.3.4/8",
            "١.2.3.4",
            " 1.2.3.4",
            "1.2.3.+4",
        ] {
            assert_eq!(parse_v4(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn v6_follows_python() {
        assert_eq!(parse_v6("::1"), Some([0, 0, 0, 0, 0, 0, 0, 1]));
        assert_eq!(
            parse_v6("fe80::1%eth0"),
            Some([0xfe80, 0, 0, 0, 0, 0, 0, 1])
        );
        assert_eq!(
            parse_v6("::ffff:1.2.3.4"),
            Some([0, 0, 0, 0, 0, 0xffff, 0x102, 0x304])
        );
        for bad in [
            "",
            ":",
            "1::2::3",
            "fe80::1%",
            "fe80::1%a%b",
            "1:2:3:4:5:6:7:8:9",
            "12345::",
            "1:2:3:4:5:6:7",
        ] {
            assert_eq!(parse_v6(bad), None, "{bad:?}");
        }
        assert!(is_link_local_v6(&parse_v6("fe80::1").unwrap()));
        assert!(is_unique_local_v6(&parse_v6("fd00::1").unwrap()));
        assert!(v4_in([172, 31, 255, 1], [172, 16, 0, 0], 12));
        assert!(!v4_in([172, 32, 0, 1], [172, 16, 0, 0], 12));
        assert!(v4_in([1, 2, 3, 4], [0, 0, 0, 0], 0));
        assert!(v4_in([0, 0, 0, 0], [0, 0, 0, 0], 32));
        assert!(!v4_in([0, 0, 0, 1], [0, 0, 0, 0], 32));
        let fe80 = parse_v6("fe80::").unwrap();
        assert!(v6_in(&parse_v6("febf::1").unwrap(), &fe80, 10));
        assert!(!v6_in(&parse_v6("fec0::1").unwrap(), &fe80, 10));
    }
}
