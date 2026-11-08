//! Finding scanners.
//!
//! Network scanners announce themselves over mDNS as `_uscan._tcp` (eSCL over
//! HTTP) or `_uscans._tcp` (over HTTPS), with a TXT record naming the model
//! (`ty`), the eSCL root (`rs`, almost always `eSCL`) and a stable `UUID`.
//! SANE devices come from `scanimage -L`. Both are asked at once, because
//! SANE probing the USB bus and the network can take several seconds on its
//! own.
//!
//! SANE's own network backends (`escl`, `airscan`) find the same network
//! scanners mDNS does. When mDNS found any, those SANE entries are dropped
//! rather than listing every printer twice — talking eSCL directly is the
//! same protocol with one less layer in between.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use super::{sane, Scanner};

/// A resolved mDNS announcement, reduced to what makes a scanner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    pub instance: String,
    pub https: bool,
    pub host: String,
    pub port: u16,
    pub txt: BTreeMap<String, String>,
}

/// An eSCL scanner from an announcement.
pub fn from_announcement(a: &Announcement) -> Scanner {
    let resource = a.txt.get("rs").map(|r| r.trim_matches('/').to_string()).unwrap_or_else(|| "eSCL".into());
    let scheme = if a.https { "https" } else { "http" };
    let host = if a.host.contains(':') { format!("[{}]", a.host) } else { a.host.clone() };
    let url = if resource.is_empty() {
        format!("{scheme}://{host}:{}", a.port)
    } else {
        format!("{scheme}://{host}:{}/{resource}", a.port)
    };
    let uuid = a.txt.get("UUID").or_else(|| a.txt.get("uuid")).cloned().filter(|u| !u.is_empty());
    let name = a.txt.get("ty").cloned().filter(|t| !t.is_empty()).unwrap_or_else(|| a.instance.clone());
    Scanner::escl(&url, uuid, &name, false)
}

/// One entry per scanner, preferring plain HTTP when a scanner offers both:
/// its HTTPS certificate cannot be checked anyway (see `escl.rs`), and HTTP
/// avoids a handshake some scanners are slow at.
pub fn merge(announcements: &[Announcement]) -> Vec<Scanner> {
    let mut by_id: BTreeMap<String, (bool, Scanner)> = BTreeMap::new();
    for a in announcements {
        let scanner = from_announcement(a);
        match by_id.get(&scanner.id) {
            Some((https, _)) if !*https => {}
            _ => {
                by_id.insert(scanner.id.clone(), (a.https, scanner));
            }
        }
    }
    by_id.into_values().map(|(_, s)| s).collect()
}

/// Browse mDNS for `wait`.
pub fn browse(wait: Duration) -> Result<Vec<Announcement>, String> {
    use mdns_sd::{ServiceDaemon, ServiceEvent};

    let daemon = ServiceDaemon::new().map_err(|e| format!("Network discovery is unavailable: {e}"))?;
    let mut receivers = Vec::new();
    for (kind, https) in [("_uscan._tcp.local.", false), ("_uscans._tcp.local.", true)] {
        if let Ok(rx) = daemon.browse(kind) {
            receivers.push((rx, https));
        }
    }
    let deadline = Instant::now() + wait;
    let mut found = Vec::new();
    while Instant::now() < deadline {
        let mut idle = true;
        for (rx, https) in &receivers {
            while let Ok(event) = rx.try_recv() {
                idle = false;
                if let ServiceEvent::ServiceResolved(info) = event {
                    // IPv4 first: link-local IPv6 needs a scope id that a URL
                    // cannot portably carry.
                    let mut addresses: Vec<String> = info.get_addresses().iter().map(|a| a.to_string()).collect();
                    addresses.sort_by_key(|a| a.contains(':'));
                    let Some(host) = addresses.into_iter().next() else { continue };
                    let txt = info
                        .get_properties()
                        .iter()
                        .map(|p| (p.key().to_string(), p.val_str().to_string()))
                        .collect();
                    let instance = info
                        .get_fullname()
                        .split("._uscan")
                        .next()
                        .unwrap_or_default()
                        .to_string();
                    found.push(Announcement { instance, https: *https, host, port: info.get_port(), txt });
                }
            }
        }
        if idle {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let _ = daemon.shutdown();
    Ok(found)
}

/// Whether a SANE device is SANE's view of a network scanner.
fn is_sane_network(device: &str) -> bool {
    device.starts_with("escl:") || device.starts_with("airscan:")
}

/// Combine what each source found. `manual` scanners always appear.
pub fn combine(network: Vec<Scanner>, sane_devices: Vec<(String, String)>, manual: &[Scanner]) -> Vec<Scanner> {
    let drop_sane_network = !network.is_empty();
    let mut out: Vec<Scanner> = Vec::new();
    let mut push = |s: Scanner| {
        if !out.iter().any(|o| o.id == s.id) {
            out.push(s);
        }
    };
    for s in manual {
        push(s.clone());
    }
    for s in network {
        push(s);
    }
    for (device, description) in sane_devices {
        if drop_sane_network && is_sane_network(&device) {
            continue;
        }
        push(Scanner::sane(&device, &description));
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Everything that can be scanned from, found within about `wait`.
pub fn discover(wait: Duration, manual: &[Scanner]) -> (Vec<Scanner>, Vec<String>) {
    let mut notes = Vec::new();
    let sane_thread = std::thread::spawn(|| if sane::available() { Some(sane::list()) } else { None });
    let network = match browse(wait) {
        Ok(found) => merge(&found),
        Err(e) => {
            notes.push(e);
            Vec::new()
        }
    };
    let sane_devices = match sane_thread.join() {
        Ok(Some(Ok(devices))) => devices,
        Ok(Some(Err(e))) => {
            notes.push(format!("SANE: {e}"));
            Vec::new()
        }
        _ => Vec::new(),
    };
    (combine(network, sane_devices, manual), notes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::Backend;

    fn ann(instance: &str, https: bool, host: &str, port: u16, txt: &[(&str, &str)]) -> Announcement {
        Announcement {
            instance: instance.into(),
            https,
            host: host.into(),
            port,
            txt: txt.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    #[test]
    fn an_announcement_becomes_an_escl_root() {
        let s = from_announcement(&ann("HP M28w [2E39]", false, "10.0.0.5", 8080, &[("rs", "/eSCL/"), ("ty", "HP LaserJet MFP M28w"), ("UUID", "ABC")]));
        assert_eq!(s.name, "HP LaserJet MFP M28w");
        assert_eq!(s.id, "escl:abc");
        assert_eq!(s.backend, Backend::Escl { url: "http://10.0.0.5:8080/eSCL".into(), uuid: Some("ABC".into()) });

        let bare = from_announcement(&ann("Brother", true, "fe80::1", 443, &[]));
        assert_eq!(bare.name, "Brother");
        assert!(matches!(bare.backend, Backend::Escl { ref url, .. } if url == "https://[fe80::1]:443/eSCL"));
    }

    #[test]
    fn a_scanner_on_both_protocols_is_listed_once_over_http() {
        let both = [
            ann("HP", true, "10.0.0.5", 443, &[("UUID", "U1")]),
            ann("HP", false, "10.0.0.5", 80, &[("UUID", "U1")]),
            ann("Canon", true, "10.0.0.6", 443, &[("UUID", "U2")]),
        ];
        let merged = merge(&both);
        assert_eq!(merged.len(), 2);
        let hp = merged.iter().find(|s| s.id == "escl:u1").unwrap();
        assert!(matches!(&hp.backend, Backend::Escl { url, .. } if url.starts_with("http://")));
    }

    #[test]
    fn sanes_copies_of_network_scanners_are_dropped_only_when_mdns_saw_them() {
        let sane = vec![
            ("airscan:e0:HP".to_string(), "eSCL HP".to_string()),
            ("epson2:libusb:001:004".to_string(), "Epson Perfection".to_string()),
        ];
        let network = vec![Scanner::escl("http://10.0.0.5/eSCL", Some("u1".into()), "HP", false)];
        let names: Vec<String> = combine(network, sane.clone(), &[]).into_iter().map(|s| s.id).collect();
        assert_eq!(names, vec!["sane:epson2:libusb:001:004", "escl:u1"]);
        // No mDNS (blocked multicast, say): SANE's network view is all there is.
        assert_eq!(combine(Vec::new(), sane, &[]).len(), 2);
    }

    /// On a network with an eSCL scanner announcing itself. Set
    /// MECLOUD_MDNS_TEST=1.
    #[test]
    fn finds_scanners_announced_on_the_network() {
        if std::env::var("MECLOUD_MDNS_TEST").as_deref() != Ok("1") {
            eprintln!("skipped: set MECLOUD_MDNS_TEST=1 on a network with an eSCL scanner");
            return;
        }
        let found = merge(&browse(Duration::from_secs(4)).unwrap());
        assert!(!found.is_empty(), "nothing announced _uscan._tcp");
        for s in &found {
            let Backend::Escl { url, .. } = &s.backend else { panic!() };
            // What was found must actually answer as a scanner.
            crate::scanner::escl::Escl::new(url).unwrap().capabilities().unwrap();
        }
    }

    #[test]
    fn manual_scanners_always_appear_and_win_over_duplicates() {
        let manual = vec![Scanner::escl("http://10.0.0.5/eSCL", Some("u1".into()), "Office", true)];
        let network = vec![Scanner::escl("http://10.0.0.5/eSCL", Some("U1".into()), "HP", false)];
        let all = combine(network, Vec::new(), &manual);
        assert_eq!(all.len(), 1);
        assert!(all[0].manual);
    }
}
