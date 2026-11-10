//! Per-IP rate limits, and who the client is behind a reverse proxy.
//!
//! The limits keep slowapi's semantics, which is what `RATE_LIMIT_*` values
//! were written against: a fixed window per (endpoint, client IP) that opens
//! on the first request, and one bucket per *endpoint* rather than per URL —
//! so a thousand downloads of a thousand different blobs share an allowance.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limit {
    pub amount: u64,
    pub window: Duration,
}

impl Limit {
    pub const fn per_minute(amount: u64) -> Limit {
        Limit {
            amount,
            window: Duration::from_secs(60),
        }
    }
}

/// `"600/minute"`, `"10 per second"`, `"100/2 hours"`, or several of them
/// separated by `;` or `,` — the `limits` library's notation.
pub fn parse_limits(text: &str) -> Result<Vec<Limit>, String> {
    let mut out = Vec::new();
    for part in text.split([';', ',']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let lower = part.to_ascii_lowercase();
        let (amount, rest) = lower
            .split_once('/')
            .or_else(|| lower.split_once(" per "))
            .ok_or_else(|| format!("{part:?} is not a rate limit like \"600/minute\""))?;
        let amount: u64 = amount
            .trim()
            .parse()
            .map_err(|_| format!("{part:?}: bad count"))?;
        let mut words = rest.split_whitespace();
        let (multiple, unit) = match (words.next(), words.next(), words.next()) {
            (Some(unit), None, None) => (1, unit),
            (Some(n), Some(unit), None) => (
                n.parse::<u64>()
                    .map_err(|_| format!("{part:?}: bad multiple"))?,
                unit,
            ),
            _ => return Err(format!("{part:?} is not a rate limit like \"600/minute\"")),
        };
        let seconds = match unit.trim_end_matches('s') {
            "second" => 1,
            "minute" => 60,
            "hour" => 3600,
            "day" => 86_400,
            "month" => 30 * 86_400,
            "year" => 365 * 86_400,
            _ => return Err(format!("{part:?}: unknown unit {unit:?}")),
        };
        if amount == 0 || multiple == 0 {
            return Err(format!("{part:?}: a limit must allow something"));
        }
        out.push(Limit {
            amount,
            window: Duration::from_secs(seconds * multiple),
        });
    }
    if out.is_empty() {
        return Err("no rate limit given".into());
    }
    Ok(out)
}

/// (endpoint, client, index of the limit).
type BucketKey = (&'static str, String, usize);

#[derive(Default)]
pub struct Limiter {
    /// -> (count, window start, window).
    buckets: Mutex<HashMap<BucketKey, (u64, Instant, Duration)>>,
}

/// Prune expired windows once the table grows past this.
const PRUNE_AT: usize = 10_000;

impl Limiter {
    /// Count one request. Err carries the Retry-After, in seconds: the length
    /// of the window that was exceeded — a ceiling on the real wait, which is
    /// the right direction to err.
    pub fn hit(&self, endpoint: &'static str, client: &str, limits: &[Limit]) -> Result<(), u64> {
        self.hit_at(endpoint, client, limits, Instant::now())
    }

    pub fn hit_at(
        &self,
        endpoint: &'static str,
        client: &str,
        limits: &[Limit],
        now: Instant,
    ) -> Result<(), u64> {
        let mut buckets = self.buckets.lock().unwrap();
        if buckets.len() > PRUNE_AT {
            buckets.retain(|_, (_, start, window)| now.duration_since(*start) < *window);
        }
        let mut exceeded = None;
        for (i, limit) in limits.iter().enumerate() {
            let entry =
                buckets
                    .entry((endpoint, client.to_string(), i))
                    .or_insert((0, now, limit.window));
            if now.duration_since(entry.1) >= limit.window {
                *entry = (0, now, limit.window);
            }
            entry.0 += 1;
            if entry.0 > limit.amount && exceeded.is_none() {
                exceeded = Some(limit.window.as_secs().max(1));
            }
        }
        match exceeded {
            Some(retry_after) => Err(retry_after),
            None => Ok(()),
        }
    }
}

/// uvicorn's `--proxy-headers` with FORWARDED_ALLOW_IPS: which peers may
/// name the client in X-Forwarded-For.
#[derive(Clone, Debug, Default)]
pub struct TrustedProxies {
    always: bool,
    hosts: Vec<IpAddr>,
    networks: Vec<(IpAddr, u8)>,
    literals: Vec<String>,
}

impl TrustedProxies {
    pub fn parse(text: &str) -> TrustedProxies {
        let mut out = TrustedProxies::default();
        for item in text.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            if item == "*" {
                out.always = true;
            } else if let Some((addr, bits)) = item.split_once('/') {
                match (addr.parse::<IpAddr>(), bits.parse::<u8>()) {
                    (Ok(ip), Ok(bits)) if bits <= max_bits(&ip) => out.networks.push((ip, bits)),
                    _ => out.literals.push(item.to_string()),
                }
            } else {
                match item.parse::<IpAddr>() {
                    Ok(ip) => out.hosts.push(ip),
                    Err(_) => out.literals.push(item.to_string()),
                }
            }
        }
        out
    }

    pub fn contains(&self, host: &str) -> bool {
        if self.always {
            return true;
        }
        match host.parse::<IpAddr>() {
            Ok(ip) => {
                let ip = canonical(ip);
                self.hosts.iter().any(|h| canonical(*h) == ip)
                    || self
                        .networks
                        .iter()
                        .any(|(net, bits)| in_network(ip, canonical(*net), *bits))
            }
            Err(_) => self.literals.iter().any(|l| l == host),
        }
    }

    /// The client address for a request from `peer`. Each proxy appends to
    /// X-Forwarded-For, so it is read right to left and the first address
    /// that is not itself a trusted proxy is the client.
    pub fn client_host(&self, peer: &str, forwarded_for: Option<&str>) -> String {
        let Some(header) = forwarded_for else {
            return peer.to_string();
        };
        if !self.contains(peer) {
            return peer.to_string();
        }
        let hosts: Vec<&str> = header
            .split(',')
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .collect();
        let Some(first) = hosts.first() else {
            return peer.to_string();
        };
        if self.always {
            return first.to_string();
        }
        for host in hosts.iter().rev() {
            if !self.contains(host) {
                return host.to_string();
            }
        }
        first.to_string()
    }
}

fn max_bits(ip: &IpAddr) -> u8 {
    if ip.is_ipv4() {
        32
    } else {
        128
    }
}

/// IPv4-mapped IPv6 (a dual-stack socket's view of an IPv4 peer) as IPv4.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

fn in_network(ip: IpAddr, net: IpAddr, bits: u8) -> bool {
    match (ip, net) {
        (IpAddr::V4(a), IpAddr::V4(n)) => {
            let mask = if bits == 0 {
                0
            } else {
                u32::MAX << (32 - bits)
            };
            u32::from(a) & mask == u32::from(n) & mask
        }
        (IpAddr::V6(a), IpAddr::V6(n)) => {
            let mask = if bits == 0 {
                0
            } else {
                u128::MAX << (128 - bits)
            };
            u128::from(a) & mask == u128::from(n) & mask
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_strings() {
        assert_eq!(
            parse_limits("600/minute").unwrap(),
            [Limit::per_minute(600)]
        );
        assert_eq!(
            parse_limits("10 per second; 100/2 hours").unwrap(),
            [
                Limit {
                    amount: 10,
                    window: Duration::from_secs(1)
                },
                Limit {
                    amount: 100,
                    window: Duration::from_secs(7200)
                }
            ]
        );
        for bad in ["", "lots", "10/fortnight", "0/minute", "x/minute"] {
            assert!(parse_limits(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_fixed_window_per_endpoint_and_client() {
        let l = Limiter::default();
        let t0 = Instant::now();
        let lim = [Limit::per_minute(2)];
        assert!(l.hit_at("a", "1.1.1.1", &lim, t0).is_ok());
        assert!(l.hit_at("a", "1.1.1.1", &lim, t0).is_ok());
        assert_eq!(l.hit_at("a", "1.1.1.1", &lim, t0), Err(60));
        // Another endpoint, another client: their own buckets.
        assert!(l.hit_at("b", "1.1.1.1", &lim, t0).is_ok());
        assert!(l.hit_at("a", "2.2.2.2", &lim, t0).is_ok());
        // The window reopens a full minute after it started.
        assert_eq!(
            l.hit_at("a", "1.1.1.1", &lim, t0 + Duration::from_secs(59)),
            Err(60)
        );
        assert!(l
            .hit_at("a", "1.1.1.1", &lim, t0 + Duration::from_secs(60))
            .is_ok());
    }

    #[test]
    fn forwarded_for_is_believed_only_from_a_trusted_peer() {
        let t = TrustedProxies::parse("127.0.0.1, 172.18.0.0/16");
        assert_eq!(t.client_host("203.0.113.9", Some("1.2.3.4")), "203.0.113.9");
        assert_eq!(t.client_host("127.0.0.1", Some("1.2.3.4")), "1.2.3.4");
        assert_eq!(
            t.client_host("172.18.0.5", Some("6.6.6.6, 1.2.3.4, 172.18.0.2")),
            "1.2.3.4"
        );
        assert_eq!(t.client_host("172.18.0.5", None), "172.18.0.5");
        assert_eq!(
            t.client_host("::ffff:127.0.0.1", Some("9.9.9.9")),
            "9.9.9.9"
        );
        assert_eq!(
            TrustedProxies::parse("*").client_host("8.8.8.8", Some("1.1.1.1, 2.2.2.2")),
            "1.1.1.1"
        );
        // All trusted: the client was a proxy too, so the leftmost wins.
        assert_eq!(
            t.client_host("127.0.0.1", Some("172.18.0.9, 127.0.0.1")),
            "172.18.0.9"
        );
    }
}
