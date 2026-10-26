//! Minimal DNS SRV query (UDP, recursion desired) to the system's DNS servers.
//! Only used for the `_minecraft._tcp.<host>` redirect (`docs/specs/title-screen-servers.md`
//! § 3.1): the first record returned is taken as is, with no priority/weight sorting.

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::Duration;

/// Wait time for a reply, per attempt.
const TIMEOUT: Duration = Duration::from_millis(1000);
/// Attempts per DNS server (one retry).
const ATTEMPTS: usize = 2;
const TYPE_SRV: u16 = 33;

/// SRV record: target (with its possible trailing dot, as returned) and port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Srv {
    pub target: String,
    pub port: u16,
}

/// First SRV record of `name`; `None` if none, on error or on timeout.
pub fn lookup_srv(name: &str) -> Option<Srv> {
    let nanos = std::time::SystemTime::UNIX_EPOCH.elapsed().map_or(0, |d| d.subsec_nanos());
    let query_id = (std::process::id() as u16) ^ (nanos >> 7) as u16;
    let query = build_query(query_id, name)?;
    for server in nameservers() {
        for _ in 0..ATTEMPTS {
            match ask(server, &query) {
                Ok(resp) => {
                    if let Some(r) = parse_response(query_id, &resp) {
                        return r;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => break,
            }
        }
    }
    None
}

fn ask(server: IpAddr, query: &[u8]) -> std::io::Result<Vec<u8>> {
    let bind: SocketAddr = if server.is_ipv4() { ([0, 0, 0, 0], 0).into() } else { ([0u16; 8], 0).into() };
    let sock = UdpSocket::bind(bind)?;
    sock.set_read_timeout(Some(TIMEOUT))?;
    sock.connect((server, 53))?;
    sock.send(query)?;
    let mut buf = vec![0u8; 4096];
    let n = sock.recv(&mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

/// System DNS servers, without duplicates, in the order of the active interfaces.
fn nameservers() -> Vec<IpAddr> {
    let mut out: Vec<IpAddr> = Vec::new();
    #[cfg(windows)]
    if let Ok(adapters) = ipconfig::get_adapters() {
        for a in adapters.iter().filter(|a| a.oper_status() == ipconfig::OperStatus::IfOperStatusUp) {
            for &ip in a.dns_servers() {
                // IPv6 site-local addresses "fec0:0:0:ffff::n": default values with no server.
                let placeholder = matches!(ip, IpAddr::V6(v6) if v6.segments()[0] == 0xfec0);
                if !placeholder && !out.contains(&ip) {
                    out.push(ip);
                }
            }
        }
    }
    #[cfg(not(windows))]
    if let Ok(conf) = std::fs::read_to_string("/etc/resolv.conf") {
        for line in conf.lines() {
            let mut it = line.split_whitespace();
            if it.next() == Some("nameserver")
                && let Some(ip) = it.next().and_then(|s| s.parse::<IpAddr>().ok())
                && !out.contains(&ip)
            {
                out.push(ip);
            }
        }
    }
    out
}

/// SRV query (class IN); `None` if the name is not encodable (empty label or > 63).
pub(crate) fn build_query(id: u16, name: &str) -> Option<Vec<u8>> {
    let mut q = Vec::with_capacity(name.len() + 18);
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]);
    for label in name.trim_end_matches('.').split('.') {
        if label.is_empty() || label.len() > 63 {
            return None;
        }
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0);
    q.extend_from_slice(&TYPE_SRV.to_be_bytes());
    q.extend_from_slice(&1u16.to_be_bytes());
    Some(q)
}

/// `None`: unusable reply (other identifier, truncated, malformed) ⇒ we retry.
/// `Some(None)`: valid reply without SRV (or nonexistent name). `Some(Some(srv))`: first SRV.
pub(crate) fn parse_response(id: u16, b: &[u8]) -> Option<Option<Srv>> {
    if b.len() < 12 || u16::from_be_bytes([b[0], b[1]]) != id || b[2] & 0x80 == 0 {
        return None;
    }
    let rcode = b[3] & 0x0f;
    if rcode == 3 {
        return Some(None);
    }
    if rcode != 0 || b[2] & 0x02 != 0 {
        return None;
    }
    let qd = u16::from_be_bytes([b[4], b[5]]);
    let an = u16::from_be_bytes([b[6], b[7]]);
    let mut pos = 12;
    for _ in 0..qd {
        pos = skip_name(b, pos)? + 4;
    }
    for _ in 0..an {
        pos = skip_name(b, pos)?;
        let h = b.get(pos..pos + 10)?;
        let ty = u16::from_be_bytes([h[0], h[1]]);
        let len = u16::from_be_bytes([h[8], h[9]]) as usize;
        let rdata = pos + 10;
        b.get(rdata..rdata + len)?;
        if ty == TYPE_SRV && len >= 7 {
            let port = u16::from_be_bytes([b[rdata + 4], b[rdata + 5]]);
            let target = read_name(b, rdata + 6)?;
            return Some(Some(Srv { target, port }));
        }
        pos = rdata + len;
    }
    Some(None)
}

/// Position after a name (labels, possibly ending with a compression pointer).
fn skip_name(b: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        let len = *b.get(pos)?;
        match len {
            0 => return Some(pos + 1),
            l if l & 0xc0 == 0xc0 => return Some(pos + 2),
            l => pos += 1 + l as usize,
        }
    }
}

/// Decompressed name, with trailing dot (form returned by resolvers: `mc.example.com.`).
fn read_name(b: &[u8], mut pos: usize) -> Option<String> {
    let mut out = String::new();
    let mut jumps = 0;
    loop {
        let len = *b.get(pos)?;
        match len {
            0 => break,
            l if l & 0xc0 == 0xc0 => {
                jumps += 1;
                if jumps > 32 {
                    return None;
                }
                pos = (((l & 0x3f) as usize) << 8) | *b.get(pos + 1)? as usize;
            }
            l => {
                let label = b.get(pos + 1..pos + 1 + l as usize)?;
                out.push_str(std::str::from_utf8(label).ok()?);
                out.push('.');
                pos += 1 + l as usize;
            }
        }
    }
    if out.is_empty() {
        out.push('.');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_and_compressed_answer() {
        let q = build_query(0x1234, "_minecraft._tcp.example.com").unwrap();
        assert_eq!(&q[12..13], &[10]);
        assert_eq!(&q[q.len() - 4..], &[0, 33, 0, 1]);
        // Reply: question copied, then an SRV whose name points to the question (0xc00c)
        // and whose target is "mc" + pointer to "example.com" (offset 12 + 11 + 5).
        let mut r = q.clone();
        r[2] = 0x81;
        r[3] = 0x80;
        r[7] = 1;
        r.extend_from_slice(&[0xc0, 12, 0, 33, 0, 1, 0, 0, 0, 60]);
        let target = [2, b'm', b'c', 0xc0, 12 + 11 + 5];
        r.extend_from_slice(&((6 + target.len()) as u16).to_be_bytes());
        r.extend_from_slice(&[0, 5, 0, 10, 0x63, 0xdd]);
        r.extend_from_slice(&target);
        let srv = parse_response(0x1234, &r).unwrap().unwrap();
        assert_eq!(srv, Srv { target: "mc.example.com.".into(), port: 25565 });
        assert_eq!(parse_response(0x9999, &r), None);
        r[3] = 0x83;
        assert_eq!(parse_response(0x1234, &r), Some(None));
    }

    #[test]
    fn bad_names() {
        assert!(build_query(1, "a..b").is_none());
        assert!(build_query(1, &"x".repeat(64)).is_none());
    }
}
