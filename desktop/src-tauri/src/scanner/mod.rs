//! Scanners: finding them, asking what they can do, and scanning.
//!
//! Two ways to reach one, because between them they cover nearly every
//! scanner someone has at home or in a small office:
//!
//! - **eSCL** (`escl.rs`), also sold as AirScan or Mopria scan: HTTP and XML
//!   over the network, implemented by practically every network scanner and
//!   multifunction printer made in the last decade. No driver, and the same on
//!   Linux, Windows and macOS. Found by mDNS (`discover.rs`), or by address.
//! - **SANE** (`sane.rs`), through the `scanimage` command: the scanner
//!   drivers Linux (and Homebrew on macOS) already has, which is how USB
//!   scanners are reached.
//!
//! Both produce pages, and `pdf.rs` turns pages into the document that is
//! uploaded. Nothing here talks to the MeCloud server; `scanning.rs` does.

pub mod discover;
pub mod escl;
pub mod pdf;
pub mod sane;

use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

/// Where the paper is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The glass. One page per scan.
    Flatbed,
    /// The document feeder, one side of each sheet.
    Feeder,
    /// The document feeder, both sides.
    Duplex,
}

impl Source {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "flatbed" | "glass" | "platen" => Some(Self::Flatbed),
            "feeder" | "adf" => Some(Self::Feeder),
            "duplex" => Some(Self::Duplex),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Color,
    Gray,
    /// One bit per pixel: the smallest file for text, and what a fax was.
    BlackWhite,
}

impl Mode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "color" | "colour" => Some(Self::Color),
            "gray" | "grey" | "grayscale" | "greyscale" => Some(Self::Gray),
            "bw" | "black_white" | "lineart" | "mono" => Some(Self::BlackWhite),
            _ => None,
        }
    }
}

/// The area to scan. The glass is usually larger than the paper on it, and
/// scanning all of it gives a page with a white margin down one side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Paper {
    A4,
    Letter,
    Legal,
    /// Everything the scanner can see.
    Full,
}

impl Paper {
    /// Width and height in millimetres, or None for the whole area.
    pub fn size_mm(self) -> Option<(f64, f64)> {
        match self {
            Paper::A4 => Some((210.0, 297.0)),
            Paper::Letter => Some((215.9, 279.4)),
            Paper::Legal => Some((215.9, 355.6)),
            Paper::Full => None,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "a4" => Some(Self::A4),
            "letter" => Some(Self::Letter),
            "legal" => Some(Self::Legal),
            "full" | "auto" => Some(Self::Full),
            _ => None,
        }
    }

    /// Letter where people use it, A4 everywhere else.
    pub fn for_locale(lang: Option<&str>) -> Self {
        let lang = lang.unwrap_or_default();
        let region = lang.split(['.', '@']).next().unwrap_or_default();
        let letter = ["en_US", "en_CA", "es_MX", "fr_CA", "es_US", "en_PH", "es_CL", "es_CO"];
        if letter.contains(&region) {
            Paper::Letter
        } else {
            Paper::A4
        }
    }
}

/// How to reach a scanner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Backend {
    /// `url` is the eSCL root, e.g. `http://192.168.1.20:80/eSCL`.
    Escl { url: String, uuid: Option<String> },
    /// A SANE device name, as `scanimage -L` prints it.
    Sane { device: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scanner {
    pub id: String,
    pub name: String,
    pub backend: Backend,
    /// Added by address rather than found on the network.
    #[serde(default)]
    pub manual: bool,
}

impl Scanner {
    pub fn escl(url: &str, uuid: Option<String>, name: &str, manual: bool) -> Self {
        let id = match &uuid {
            Some(u) => format!("escl:{}", u.to_ascii_lowercase()),
            None => format!("escl:{url}"),
        };
        Self { id, name: name.to_string(), backend: Backend::Escl { url: url.to_string(), uuid }, manual }
    }

    pub fn sane(device: &str, name: &str) -> Self {
        Self {
            id: format!("sane:{device}"),
            name: name.to_string(),
            backend: Backend::Sane { device: device.to_string() },
            manual: false,
        }
    }

    /// One line for the user: what kind of connection this is.
    pub fn via(&self) -> &'static str {
        match self.backend {
            Backend::Escl { .. } => "network",
            Backend::Sane { .. } => "SANE driver",
        }
    }
}

/// What one input source can do.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceOptions {
    pub source: Source,
    pub modes: Vec<Mode>,
    pub resolutions: Vec<u32>,
    /// The largest area it scans, when it says.
    pub max_mm: Option<(f64, f64)>,
}

/// What a scanner can do.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct Options {
    pub sources: Vec<SourceOptions>,
}

impl Options {
    pub fn source(&self, source: Source) -> Option<&SourceOptions> {
        self.sources.iter().find(|s| s.source == source)
    }
}

/// What to scan, as the user chose it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub source: Source,
    pub mode: Mode,
    pub resolution: u32,
    pub paper: Paper,
}

/// The resolutions worth offering: the usual document ones the scanner can
/// do. A scanner reporting "1 to 1200 in steps of 1" does not need a
/// 1200-entry menu.
pub const USUAL_RESOLUTIONS: [u32; 5] = [150, 200, 300, 400, 600];

pub fn offered_resolutions(supported: &[u32], range: Option<(u32, u32)>) -> Vec<u32> {
    let mut out: Vec<u32> = USUAL_RESOLUTIONS
        .iter()
        .copied()
        .filter(|r| supported.contains(r) || range.is_some_and(|(lo, hi)| (lo..=hi).contains(r)))
        .collect();
    if out.is_empty() {
        // An unusual scanner: offer what it said, within reason.
        out = supported.iter().copied().filter(|r| (75..=1200).contains(r)).collect();
        out.sort_unstable();
        out.dedup();
    }
    out
}

/// Settings the scanner will accept: the requested ones where it can do
/// them, its nearest equivalent where it cannot. A remembered setting for a
/// scanner that has since been swapped must not turn into a refused job.
pub fn fit(options: &Options, wanted: Settings) -> Result<Settings, String> {
    let source = options
        .source(wanted.source)
        .or_else(|| options.sources.first())
        .ok_or("This scanner did not say how it can scan.")?;
    let mode = if source.modes.contains(&wanted.mode) {
        wanted.mode
    } else {
        // Prefer losing colour to gaining it: grey rather than colour for a
        // black-and-white request, and grey for colour on a grey-only device.
        [Mode::Gray, Mode::Color, Mode::BlackWhite]
            .into_iter()
            .find(|m| source.modes.contains(m))
            .ok_or("This scanner did not say which colour modes it has.")?
    };
    let resolution = source
        .resolutions
        .iter()
        .copied()
        .min_by_key(|r| (*r as i64 - wanted.resolution as i64).abs())
        .unwrap_or(wanted.resolution);
    Ok(Settings { source: source.source, mode, resolution, paper: wanted.paper })
}

/// The scan area in millimetres: the paper size, cut to what the scanner can
/// reach.
pub fn area_mm(paper: Paper, max: Option<(f64, f64)>) -> Option<(f64, f64)> {
    match (paper.size_mm(), max) {
        (Some((w, h)), Some((mw, mh))) => Some((w.min(mw), h.min(mh))),
        (Some(size), None) => Some(size),
        (None, max) => max,
    }
}

/// What came back from a scan.
#[derive(Debug, Clone)]
pub enum Scanned {
    /// Pictures, one per page side, to be made into a PDF here.
    Pages(Vec<pdf::Page>),
    /// A finished PDF, from a scanner that offers nothing else.
    Pdf(Vec<u8>),
}

/// Ask a scanner what it can do.
pub fn options(scanner: &Scanner) -> Result<Options, String> {
    match &scanner.backend {
        Backend::Escl { url, .. } => escl::Escl::new(url)?.options(),
        Backend::Sane { device } => sane::options(device),
    }
}

/// Scan. `progress` is called with a line for the user as things happen;
/// `cancel` stops between pages (and kills `scanimage`).
pub fn scan(
    scanner: &Scanner, settings: Settings, progress: &dyn Fn(&str), cancel: &AtomicBool,
) -> Result<Scanned, String> {
    match &scanner.backend {
        Backend::Escl { url, .. } => escl::Escl::new(url)?.scan(settings, progress, cancel),
        Backend::Sane { device } => sane::scan(device, settings, progress, cancel),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(modes: &[Mode], resolutions: &[u32]) -> Options {
        Options {
            sources: vec![SourceOptions {
                source: Source::Flatbed,
                modes: modes.to_vec(),
                resolutions: resolutions.to_vec(),
                max_mm: Some((216.0, 297.0)),
            }],
        }
    }

    #[test]
    fn a_setting_the_scanner_lacks_falls_back_to_the_nearest() {
        let options = caps(&[Mode::Gray, Mode::Color], &[150, 300, 600]);
        let wanted = Settings { source: Source::Feeder, mode: Mode::BlackWhite, resolution: 400, paper: Paper::A4 };
        let got = fit(&options, wanted).unwrap();
        assert_eq!(got.source, Source::Flatbed);
        assert_eq!(got.mode, Mode::Gray);
        assert!(got.resolution == 300 || got.resolution == 600);
        assert!(fit(&Options::default(), wanted).is_err());
    }

    #[test]
    fn only_usual_resolutions_are_offered() {
        assert_eq!(offered_resolutions(&[], Some((1, 1200))), vec![150, 200, 300, 400, 600]);
        assert_eq!(offered_resolutions(&[75, 150, 300, 1200], None), vec![150, 300]);
        assert_eq!(offered_resolutions(&[100, 250], None), vec![100, 250]);
    }

    #[test]
    fn the_scan_area_never_exceeds_the_glass() {
        assert_eq!(area_mm(Paper::Legal, Some((216.0, 297.0))), Some((215.9, 297.0)));
        assert_eq!(area_mm(Paper::Full, Some((216.0, 297.0))), Some((216.0, 297.0)));
        assert_eq!(area_mm(Paper::A4, None), Some((210.0, 297.0)));
    }

    #[test]
    fn paper_follows_the_locale() {
        assert_eq!(Paper::for_locale(Some("en_US.UTF-8")), Paper::Letter);
        assert_eq!(Paper::for_locale(Some("en_GB.UTF-8")), Paper::A4);
        assert_eq!(Paper::for_locale(Some("de_DE@euro")), Paper::A4);
        assert_eq!(Paper::for_locale(None), Paper::A4);
    }

    #[test]
    fn ids_are_stable_across_addresses() {
        // A network scanner that moves to a new address is the same scanner.
        let a = Scanner::escl("http://10.0.0.5/eSCL", Some("ABC-1".into()), "HP", false);
        let b = Scanner::escl("http://10.0.0.9/eSCL", Some("abc-1".into()), "HP", false);
        assert_eq!(a.id, b.id);
        assert_eq!(Scanner::sane("test:0", "Test").id, "sane:test:0");
    }
}
