//! eSCL ("AirScan", "Mopria scan"): driverless network scanning.
//!
//! The protocol is plain HTTP under a root the scanner advertises (almost
//! always `/eSCL`):
//!
//! | Request | Answer |
//! |---|---|
//! | `GET  ScannerCapabilities` | XML: sources, colour modes, resolutions, formats |
//! | `GET  ScannerStatus` | XML: idle or busy, and whether the feeder has paper |
//! | `POST ScanJobs` + settings XML | `201 Created`, `Location:` the job |
//! | `GET  {job}/NextDocument` | one page (or the whole PDF); `404` when done |
//! | `DELETE {job}` | job released |
//!
//! Pages are asked for as JPEG and made into a PDF here (see `pdf.rs`), which
//! is what every eSCL scanner can produce and what lets a page be added to a
//! document later. A scanner that offers only PDF gets asked for PDF.
//!
//! Scanners on `https` (`_uscans`) present self-signed certificates — a
//! printer has no way to get any other kind — so their certificate is not
//! checked. That is the same protection as the plain-HTTP address the same
//! scanner also offers, which is preferred whenever both are advertised.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use quick_xml::events::Event;
use quick_xml::Reader;
use url::Url;

use super::pdf::{ImageKind, Page};
use super::{area_mm, offered_resolutions, Mode, Options, Scanned, Settings, Source, SourceOptions};

pub struct Escl {
    root: Url,
    http: reqwest::blocking::Client,
}

/// eSCL measures in three-hundredths of an inch.
fn mm_to_300ths(mm: f64) -> u32 {
    (mm / 25.4 * 300.0).round() as u32
}

fn from_300ths(units: u32) -> f64 {
    units as f64 * 25.4 / 300.0
}

/// One input source's section of the capabilities document.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Section {
    pub modes: Vec<String>,
    pub formats: Vec<String>,
    pub resolutions: Vec<u32>,
    pub range: Option<(u32, u32)>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
}

/// The capabilities document, reduced to what this client uses.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Capabilities {
    pub version: Option<String>,
    pub make_and_model: Option<String>,
    pub uuid: Option<String>,
    pub platen: Option<Section>,
    pub adf_simplex: Option<Section>,
    pub adf_duplex: Option<Section>,
    /// `<scan:AdfOptions><scan:AdfOption>Duplex</…>` with no separate duplex
    /// section: the simplex capabilities apply to both sides.
    pub adf_duplex_option: bool,
}

fn local(name: &str) -> String {
    name.rsplit(':').next().unwrap_or_default().to_string()
}

/// Parse `ScannerCapabilities`.
///
/// Namespaces are ignored, deliberately: scanners disagree on prefixes (and
/// a few on namespace URIs), but never on element names.
pub fn parse_capabilities(xml: &str) -> Result<Capabilities, String> {
    let mut reader = Reader::from_str(xml);
    let mut stack: Vec<String> = Vec::new();
    let mut caps = Capabilities::default();
    let mut current: Option<(&'static str, Section)> = None;
    let mut range_min: Option<u32> = None;

    loop {
        match reader.read_event() {
            Err(e) => return Err(format!("The scanner described itself in XML this client could not read: {e}")),
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                let name = local(e.name().into_inner());
                match name.as_str() {
                    "PlatenInputCaps" => current = Some(("platen", Section::default())),
                    "AdfSimplexInputCaps" => current = Some(("simplex", Section::default())),
                    "AdfDuplexInputCaps" => current = Some(("duplex", Section::default())),
                    _ => {}
                }
                stack.push(name);
            }
            Ok(Event::End(e)) => {
                let name = local(e.name().into_inner());
                if matches!(name.as_str(), "PlatenInputCaps" | "AdfSimplexInputCaps" | "AdfDuplexInputCaps") {
                    if let Some((which, section)) = current.take() {
                        match which {
                            "platen" => caps.platen = Some(section),
                            "simplex" => caps.adf_simplex = Some(section),
                            _ => caps.adf_duplex = Some(section),
                        }
                    }
                }
                stack.pop();
            }
            Ok(Event::Text(t)) => {
                let text = t.xml10_content().trim().to_string();
                if text.is_empty() {
                    continue;
                }
                let Some(name) = stack.last().map(String::as_str) else { continue };
                let parent = stack.len().checked_sub(2).and_then(|i| stack.get(i)).map(String::as_str);
                match (name, &mut current) {
                    ("Version", None) if stack.len() == 2 => caps.version = Some(text),
                    ("MakeAndModel", None) => caps.make_and_model = Some(text),
                    ("UUID", None) => caps.uuid = Some(text),
                    ("AdfOption", None) if text.eq_ignore_ascii_case("Duplex") => caps.adf_duplex_option = true,
                    ("ColorMode", Some((_, s))) => {
                        if !s.modes.contains(&text) {
                            s.modes.push(text)
                        }
                    }
                    ("DocumentFormat" | "DocumentFormatExt", Some((_, s))) => {
                        let format = text.to_ascii_lowercase();
                        if !s.formats.contains(&format) {
                            s.formats.push(format)
                        }
                    }
                    ("MaxWidth", Some((_, s))) => s.max_width = text.parse().ok(),
                    ("MaxHeight", Some((_, s))) => s.max_height = text.parse().ok(),
                    ("XResolution", Some((_, s))) if parent == Some("DiscreteResolution") => {
                        if let Ok(r) = text.parse::<u32>() {
                            if !s.resolutions.contains(&r) {
                                s.resolutions.push(r)
                            }
                        }
                    }
                    ("Min", Some(_)) if parent == Some("XResolutionRange") => range_min = text.parse().ok(),
                    ("Max", Some((_, s))) if parent == Some("XResolutionRange") => {
                        if let (Some(lo), Ok(hi)) = (range_min.take(), text.parse::<u32>()) {
                            s.range = Some((lo, hi));
                        }
                    }
                    _ => {}
                }
            }
            Ok(_) => {}
        }
    }
    if caps.platen.is_none() && caps.adf_simplex.is_none() && caps.adf_duplex.is_none() {
        return Err("The scanner did not describe any way to scan.".into());
    }
    Ok(caps)
}

fn mode_from_escl(value: &str) -> Option<Mode> {
    match value {
        "RGB24" | "RGB48" => Some(Mode::Color),
        "Grayscale8" | "Grayscale16" => Some(Mode::Gray),
        "BlackAndWhite1" => Some(Mode::BlackWhite),
        _ => None,
    }
}

/// The eSCL name for a mode, from what the section lists.
fn escl_mode(section: &Section, mode: Mode) -> String {
    let preferred: &[&str] = match mode {
        Mode::Color => &["RGB24", "RGB48"],
        Mode::Gray => &["Grayscale8", "Grayscale16"],
        Mode::BlackWhite => &["BlackAndWhite1"],
    };
    preferred
        .iter()
        .find(|p| section.modes.iter().any(|m| m == *p))
        .unwrap_or(&preferred[0])
        .to_string()
}

impl Capabilities {
    fn section(&self, source: Source) -> Option<&Section> {
        match source {
            Source::Flatbed => self.platen.as_ref(),
            Source::Feeder => self.adf_simplex.as_ref(),
            Source::Duplex => self.adf_duplex.as_ref().or(if self.adf_duplex_option { self.adf_simplex.as_ref() } else { None }),
        }
    }

    pub fn options(&self) -> Options {
        let mut sources = Vec::new();
        for source in [Source::Flatbed, Source::Feeder, Source::Duplex] {
            let Some(section) = self.section(source) else { continue };
            // Black and white is not offered over eSCL. BlackAndWhite1 comes
            // back as PDF or as a format this client would have to decode, and
            // a "black and white" that quietly arrived as grey would be a
            // promise not kept. Grey is the honest choice for text.
            let mut modes: Vec<Mode> = section
                .modes
                .iter()
                .filter_map(|m| mode_from_escl(m))
                .filter(|m| *m != Mode::BlackWhite)
                .collect();
            modes.sort();
            modes.dedup();
            let max_mm = match (section.max_width, section.max_height) {
                (Some(w), Some(h)) => Some((from_300ths(w), from_300ths(h))),
                _ => None,
            };
            sources.push(SourceOptions {
                source,
                modes,
                resolutions: offered_resolutions(&section.resolutions, section.range),
                max_mm,
            });
        }
        Options { sources }
    }
}

/// The document format to ask for: JPEG pages where possible, PDF if that is
/// all there is.
fn pick_format(section: &Section) -> &'static str {
    let has = |f: &str| section.formats.iter().any(|x| x == f);
    if has("image/jpeg") || !has("application/pdf") {
        "image/jpeg"
    } else {
        "application/pdf"
    }
}

/// The `ScanSettings` document for a job.
pub fn scan_settings_xml(caps: &Capabilities, settings: Settings) -> Result<(String, &'static str), String> {
    let section = caps.section(settings.source).ok_or("This scanner cannot scan from there.")?;
    let format = pick_format(section);
    let (width_mm, height_mm) = area_mm(
        settings.paper,
        match (section.max_width, section.max_height) {
            (Some(w), Some(h)) => Some((from_300ths(w), from_300ths(h))),
            _ => None,
        },
    )
    .unwrap_or((210.0, 297.0));
    // Not offered (see `options`), but a remembered setting or the command
    // line can still ask: grey JPEG is the portable answer.
    let mode = if settings.mode == Mode::BlackWhite && format == "image/jpeg" {
        escl_mode(section, Mode::Gray)
    } else {
        escl_mode(section, settings.mode)
    };
    let input = match settings.source {
        Source::Flatbed => "Platen",
        _ => "Feeder",
    };
    let duplex = if settings.source == Source::Duplex { "\n  <scan:Duplex>true</scan:Duplex>" } else { "" };
    let version = caps.version.clone().unwrap_or_else(|| "2.0".into());
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<scan:ScanSettings xmlns:scan="http://schemas.hp.com/imaging/escl/2011/05/03" xmlns:pwg="http://www.pwg.org/schemas/2010/12/sm">
  <pwg:Version>{version}</pwg:Version>
  <scan:Intent>Document</scan:Intent>
  <pwg:ScanRegions pwg:MustHonor="false">
    <pwg:ScanRegion>
      <pwg:ContentRegionUnits>escl:ThreeHundredthsOfInches</pwg:ContentRegionUnits>
      <pwg:XOffset>0</pwg:XOffset>
      <pwg:YOffset>0</pwg:YOffset>
      <pwg:Width>{w}</pwg:Width>
      <pwg:Height>{h}</pwg:Height>
    </pwg:ScanRegion>
  </pwg:ScanRegions>
  <pwg:InputSource>{input}</pwg:InputSource>{duplex}
  <scan:ColorMode>{mode}</scan:ColorMode>
  <scan:XResolution>{r}</scan:XResolution>
  <scan:YResolution>{r}</scan:YResolution>
  <pwg:DocumentFormat>{format}</pwg:DocumentFormat>
  <scan:DocumentFormatExt>{format}</scan:DocumentFormatExt>
</scan:ScanSettings>
"#,
        w = mm_to_300ths(width_mm),
        h = mm_to_300ths(height_mm),
        r = settings.resolution,
    );
    Ok((xml, format))
}

/// What `ScannerStatus` says.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub state: Option<String>,
    pub adf: Option<String>,
}

pub fn parse_status(xml: &str) -> Status {
    let mut reader = Reader::from_str(xml);
    let mut stack: Vec<String> = Vec::new();
    let mut status = Status::default();
    loop {
        match reader.read_event() {
            Err(_) | Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => stack.push(local(e.name().into_inner())),
            Ok(Event::End(_)) => {
                stack.pop();
            }
            Ok(Event::Text(t)) => {
                let text = t.xml10_content().trim().to_string();
                // Jobs in the status document have a State of their own.
                if stack.iter().any(|s| s == "JobInfo") {
                    continue;
                }
                match stack.last().map(String::as_str) {
                    Some("State") => status.state = Some(text),
                    Some("AdfState") => status.adf = Some(text),
                    _ => {}
                }
            }
            Ok(_) => {}
        }
    }
    status
}

/// Normalise what someone typed as a scanner's address into an eSCL root.
///
/// `192.168.1.20`, `printer.local`, `http://printer:8080` and
/// `https://printer/eSCL/` all end up as a root URL ending in the resource
/// path, with `/eSCL` assumed when none is given.
pub fn root_from_input(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Enter the scanner's address.".into());
    }
    let with_scheme = if trimmed.contains("://") { trimmed.to_string() } else { format!("http://{trimmed}") };
    let mut url = Url::parse(&with_scheme).map_err(|_| format!("\"{trimmed}\" is not an address."))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(format!("\"{trimmed}\" is not an address."));
    }
    if url.path() == "/" || url.path().is_empty() {
        url.set_path("/eSCL");
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.as_str().trim_end_matches('/').to_string())
}

/// How long to wait for one page. A scanner answers `NextDocument` once the
/// page is scanned, and 600 dpi colour on a slow scanner takes its time.
const PAGE_TIMEOUT: Duration = Duration::from_secs(180);
/// How long to keep asking a scanner that says "not ready yet" (503).
const NOT_READY_FOR: Duration = Duration::from_secs(120);
/// Pages, after which a feeder that never says it is empty is not believed.
const MAX_PAGES: usize = 500;

impl Escl {
    pub fn new(root: &str) -> Result<Self, String> {
        let mut root = Url::parse(root).map_err(|e| format!("Bad scanner address {root}: {e}"))?;
        // A trailing slash makes `join` treat the root as a directory.
        if !root.path().ends_with('/') {
            let path = format!("{}/", root.path());
            root.set_path(&path);
        }
        let http = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(PAGE_TIMEOUT)
            .user_agent(concat!("MeCloud-Desktop/", env!("CARGO_PKG_VERSION")))
            // See the module comment: printers' certificates are self-signed.
            .danger_accept_invalid_certs(root.scheme() == "https")
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { root, http })
    }

    fn url(&self, path: &str) -> Result<Url, String> {
        self.root.join(path).map_err(|e| e.to_string())
    }

    fn get_text(&self, path: &str) -> Result<String, String> {
        let response = self
            .http
            .get(self.url(path)?)
            .timeout(Duration::from_secs(15))
            .send()
            .map_err(|e| format!("Could not reach the scanner: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("The scanner refused {path} ({}).", response.status()));
        }
        response.text().map_err(|e| e.to_string())
    }

    pub fn capabilities(&self) -> Result<Capabilities, String> {
        parse_capabilities(&self.get_text("ScannerCapabilities")?)
    }

    pub fn options(&self) -> Result<Options, String> {
        Ok(self.capabilities()?.options())
    }

    pub fn status(&self) -> Result<Status, String> {
        Ok(parse_status(&self.get_text("ScannerStatus")?))
    }

    pub fn scan(&self, settings: Settings, progress: &dyn Fn(&str), cancel: &AtomicBool) -> Result<Scanned, String> {
        let caps = self.capabilities()?;
        let (body, format) = scan_settings_xml(&caps, settings)?;

        // Asking first gives a clear answer for the two things that most often
        // go wrong, where the job itself would just be refused with a 409.
        if let Ok(status) = self.status() {
            if settings.source != Source::Flatbed && status.adf.as_deref() == Some("ScannerAdfEmpty") {
                return Err("The document feeder is empty. Put the pages in and try again.".into());
            }
            if let Some(state) = status.state.as_deref() {
                if state != "Idle" && state != "Processing" {
                    return Err(format!("The scanner is not ready ({state})."));
                }
            }
        }

        progress("Starting the scanner…");
        let response = self
            .http
            .post(self.url("ScanJobs")?)
            .header("Content-Type", "text/xml")
            .body(body)
            .timeout(Duration::from_secs(30))
            .send()
            .map_err(|e| format!("Could not reach the scanner: {e}"))?;
        let status = response.status();
        if status == reqwest::StatusCode::CONFLICT || status == reqwest::StatusCode::SERVICE_UNAVAILABLE {
            return Err(if settings.source == Source::Flatbed {
                "The scanner is busy. Try again in a moment.".into()
            } else {
                "The scanner refused the job — is there paper in the feeder?".into()
            });
        }
        if status != reqwest::StatusCode::CREATED && !status.is_success() {
            return Err(format!("The scanner refused the job ({status})."));
        }
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or("The scanner accepted the job but did not say where it is.")?;
        // Absolute or relative to the scanner: both are seen in practice.
        let mut job = self.root.join(location).map_err(|e| e.to_string())?;
        if !job.path().ends_with('/') {
            let path = format!("{}/", job.path());
            job.set_path(&path);
        }

        let result = self.collect(&job, settings.source, format, settings.resolution, progress, cancel);
        // Released whatever happened, or some scanners stay busy until they
        // time the job out themselves.
        let _ = self.http.delete(job.as_str().trim_end_matches('/')).timeout(Duration::from_secs(10)).send();
        result
    }

    fn collect(
        &self, job: &Url, source: Source, format: &str, dpi: u32, progress: &dyn Fn(&str), cancel: &AtomicBool,
    ) -> Result<Scanned, String> {
        let next = job.join("NextDocument").map_err(|e| e.to_string())?;
        let mut pages: Vec<Page> = Vec::new();
        let mut not_ready_since: Option<Instant> = None;

        while pages.len() < MAX_PAGES {
            if cancel.load(Ordering::Relaxed) {
                return Err("Cancelled.".into());
            }
            // The feeder's last page is only known to be the last when the
            // next request comes back empty, so after the first page this
            // counts what has arrived rather than promising another.
            progress(&match (source, pages.len()) {
                (Source::Flatbed, _) => "Scanning…".to_string(),
                (_, 0) => "Scanning page 1…".to_string(),
                (_, 1) => "Scanning… 1 page so far".to_string(),
                (_, n) => format!("Scanning… {n} pages so far"),
            });
            let response = self.http.get(next.clone()).send().map_err(|e| format!("The scanner stopped answering: {e}"))?;
            let status = response.status();
            if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE {
                break; // no more pages
            }
            if status == reqwest::StatusCode::SERVICE_UNAVAILABLE {
                let since = *not_ready_since.get_or_insert_with(Instant::now);
                if since.elapsed() > NOT_READY_FOR {
                    return Err("The scanner never finished the page.".into());
                }
                std::thread::sleep(Duration::from_millis(800));
                continue;
            }
            if !status.is_success() {
                return Err(format!("The scanner failed while scanning ({status})."));
            }
            not_ready_since = None;
            let bytes = response.bytes().map_err(|e| format!("The scanner stopped part way through a page: {e}"))?.to_vec();
            if format == "application/pdf" || bytes.starts_with(b"%PDF") {
                // A PDF is the whole document in one go.
                return Ok(Scanned::Pdf(bytes));
            }
            pages.push(Page { kind: ImageKind::Jpeg, bytes, dpi });
            // The glass holds one page. Asking a flatbed for a second makes
            // some scanners start scanning again.
            if source == Source::Flatbed {
                break;
            }
        }
        if pages.is_empty() {
            return Err(if source == Source::Flatbed {
                "The scanner finished without sending a page.".into()
            } else {
                "No pages came through the feeder. Put the pages in and try again.".into()
            });
        }
        Ok(Scanned::Pages(pages))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::scanner::Paper;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// Trimmed from a real HP LaserJet MFP's capabilities, with an ADF.
    pub const HP_CAPS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<scan:ScannerCapabilities xmlns:pwg="http://www.pwg.org/schemas/2010/12/sm" xmlns:scan="http://schemas.hp.com/imaging/escl/2011/05/03">
  <pwg:Version>2.63</pwg:Version>
  <pwg:MakeAndModel>HP LaserJet MFP M28w</pwg:MakeAndModel>
  <scan:UUID>564E4243-4E38-3833-3138-a0481c4b2e39</scan:UUID>
  <scan:Platen>
    <scan:PlatenInputCaps>
      <scan:MinWidth>8</scan:MinWidth>
      <scan:MaxWidth>2550</scan:MaxWidth>
      <scan:MinHeight>8</scan:MinHeight>
      <scan:MaxHeight>3508</scan:MaxHeight>
      <scan:SettingProfiles>
        <scan:SettingProfile>
          <scan:ColorModes>
            <scan:ColorMode>RGB24</scan:ColorMode>
            <scan:ColorMode>Grayscale8</scan:ColorMode>
          </scan:ColorModes>
          <scan:DocumentFormats>
            <pwg:DocumentFormat>application/pdf</pwg:DocumentFormat>
            <pwg:DocumentFormat>image/jpeg</pwg:DocumentFormat>
            <scan:DocumentFormatExt>application/pdf</scan:DocumentFormatExt>
            <scan:DocumentFormatExt>image/jpeg</scan:DocumentFormatExt>
          </scan:DocumentFormats>
          <scan:SupportedResolutions>
            <scan:DiscreteResolutions>
              <scan:DiscreteResolution><scan:XResolution>75</scan:XResolution><scan:YResolution>75</scan:YResolution></scan:DiscreteResolution>
              <scan:DiscreteResolution><scan:XResolution>150</scan:XResolution><scan:YResolution>150</scan:YResolution></scan:DiscreteResolution>
              <scan:DiscreteResolution><scan:XResolution>300</scan:XResolution><scan:YResolution>300</scan:YResolution></scan:DiscreteResolution>
              <scan:DiscreteResolution><scan:XResolution>600</scan:XResolution><scan:YResolution>600</scan:YResolution></scan:DiscreteResolution>
            </scan:DiscreteResolutions>
          </scan:SupportedResolutions>
        </scan:SettingProfile>
      </scan:SettingProfiles>
    </scan:PlatenInputCaps>
  </scan:Platen>
  <scan:Adf>
    <scan:AdfSimplexInputCaps>
      <scan:MaxWidth>2550</scan:MaxWidth>
      <scan:MaxHeight>4200</scan:MaxHeight>
      <scan:SettingProfiles>
        <scan:SettingProfile>
          <scan:ColorModes><scan:ColorMode>RGB24</scan:ColorMode><scan:ColorMode>Grayscale8</scan:ColorMode><scan:ColorMode>BlackAndWhite1</scan:ColorMode></scan:ColorModes>
          <scan:DocumentFormats><pwg:DocumentFormat>image/jpeg</pwg:DocumentFormat></scan:DocumentFormats>
          <scan:SupportedResolutions>
            <scan:ResolutionRange>
              <scan:XResolutionRange><scan:Min>100</scan:Min><scan:Max>300</scan:Max><scan:Step>1</scan:Step></scan:XResolutionRange>
              <scan:YResolutionRange><scan:Min>100</scan:Min><scan:Max>300</scan:Max><scan:Step>1</scan:Step></scan:YResolutionRange>
            </scan:ResolutionRange>
          </scan:SupportedResolutions>
        </scan:SettingProfile>
      </scan:SettingProfiles>
    </scan:AdfSimplexInputCaps>
    <scan:AdfOptions><scan:AdfOption>Duplex</scan:AdfOption></scan:AdfOptions>
  </scan:Adf>
</scan:ScannerCapabilities>"#;

    #[test]
    fn reads_capabilities() {
        let caps = parse_capabilities(HP_CAPS).unwrap();
        assert_eq!(caps.version.as_deref(), Some("2.63"));
        assert_eq!(caps.make_and_model.as_deref(), Some("HP LaserJet MFP M28w"));
        let platen = caps.platen.as_ref().unwrap();
        assert_eq!(platen.modes, vec!["RGB24", "Grayscale8"]);
        assert_eq!(platen.resolutions, vec![75, 150, 300, 600]);
        assert_eq!((platen.max_width, platen.max_height), (Some(2550), Some(3508)));
        assert!(platen.formats.contains(&"image/jpeg".to_string()));
        assert_eq!(caps.adf_simplex.as_ref().unwrap().range, Some((100, 300)));
        assert!(caps.adf_duplex_option);

        let options = caps.options();
        let sources: Vec<Source> = options.sources.iter().map(|s| s.source).collect();
        assert_eq!(sources, vec![Source::Flatbed, Source::Feeder, Source::Duplex]);
        assert_eq!(options.source(Source::Flatbed).unwrap().modes, vec![Mode::Color, Mode::Gray]);
        assert_eq!(options.source(Source::Feeder).unwrap().resolutions, vec![150, 200, 300]);
        let (w, h) = options.source(Source::Flatbed).unwrap().max_mm.unwrap();
        assert!((w - 215.9).abs() < 0.1 && (h - 297.0).abs() < 0.1);
    }

    #[test]
    fn a_document_with_no_sources_is_refused() {
        assert!(parse_capabilities("<scan:ScannerCapabilities/>").is_err());
        assert!(parse_capabilities("<a><b></a>").is_err());
    }

    #[test]
    fn settings_ask_for_jpeg_pages_of_the_chosen_paper() {
        let caps = parse_capabilities(HP_CAPS).unwrap();
        let settings = Settings { source: Source::Flatbed, mode: Mode::Color, resolution: 300, paper: Paper::A4 };
        let (xml, format) = scan_settings_xml(&caps, settings).unwrap();
        assert_eq!(format, "image/jpeg");
        assert!(xml.contains("<pwg:Version>2.63</pwg:Version>"));
        assert!(xml.contains("<pwg:InputSource>Platen</pwg:InputSource>"));
        assert!(xml.contains("<scan:ColorMode>RGB24</scan:ColorMode>"));
        assert!(xml.contains("<pwg:Width>2480</pwg:Width>"), "A4 width in 1/300 in");
        assert!(xml.contains("<pwg:Height>3508</pwg:Height>"));
        assert!(!xml.contains("Duplex"));

        let duplex = Settings { source: Source::Duplex, mode: Mode::BlackWhite, resolution: 200, paper: Paper::Legal };
        let (xml, _) = scan_settings_xml(&caps, duplex).unwrap();
        assert!(xml.contains("<pwg:InputSource>Feeder</pwg:InputSource>"));
        assert!(xml.contains("<scan:Duplex>true</scan:Duplex>"));
        // Black and white travels as grey JPEG.
        assert!(xml.contains("<scan:ColorMode>Grayscale8</scan:ColorMode>"));
        assert!(xml.contains("<pwg:Height>4200</pwg:Height>"), "legal, limited to the feeder's reach");
    }

    #[test]
    fn a_pdf_only_scanner_is_asked_for_pdf() {
        let mut caps = parse_capabilities(HP_CAPS).unwrap();
        caps.platen.as_mut().unwrap().formats = vec!["application/pdf".into()];
        let settings = Settings { source: Source::Flatbed, mode: Mode::Color, resolution: 300, paper: Paper::A4 };
        assert_eq!(scan_settings_xml(&caps, settings).unwrap().1, "application/pdf");
    }

    #[test]
    fn reads_status_but_not_a_jobs_state() {
        let xml = r#"<scan:ScannerStatus xmlns:scan="x" xmlns:pwg="y"><pwg:Version>2.6</pwg:Version>
            <pwg:State>Idle</pwg:State><scan:AdfState>ScannerAdfEmpty</scan:AdfState>
            <scan:Jobs><scan:JobInfo><pwg:JobState>Completed</pwg:JobState><pwg:State>Processing</pwg:State></scan:JobInfo></scan:Jobs>
            </scan:ScannerStatus>"#;
        assert_eq!(parse_status(xml), Status { state: Some("Idle".into()), adf: Some("ScannerAdfEmpty".into()) });
    }

    #[test]
    fn addresses_typed_by_hand() {
        assert_eq!(root_from_input("192.168.1.20").unwrap(), "http://192.168.1.20/eSCL");
        assert_eq!(root_from_input("http://printer:8080").unwrap(), "http://printer:8080/eSCL");
        assert_eq!(root_from_input(" https://printer/eSCL/ ").unwrap(), "https://printer/eSCL");
        assert_eq!(root_from_input("printer.local/scan").unwrap(), "http://printer.local/scan");
        assert!(root_from_input("").is_err());
        assert!(root_from_input("ftp://x").is_err());
    }

    /// A scripted eSCL scanner on a loopback port.
    pub struct FakeScanner {
        pub root: String,
        pub requests: Arc<Mutex<Vec<String>>>,
    }

    /// `pages` JPEGs come out of the feeder (or one off the glass);
    /// `adf_state` is what ScannerStatus reports; `not_ready` 503s precede
    /// each page.
    pub fn fake_scanner(pages: usize, adf_state: &'static str, not_ready: usize) -> FakeScanner {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        std::thread::spawn(move || {
            let mut served = 0usize;
            let mut waits = 0usize;
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let mut length = 0usize;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; length];
                reader.read_exact(&mut body).unwrap();
                let request = line.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
                log.lock().unwrap().push(format!("{request} {}", String::from_utf8_lossy(&body)));

                let (status, headers, payload): (&str, String, Vec<u8>) = if request == "GET /eSCL/ScannerCapabilities" {
                    ("200 OK", "Content-Type: text/xml\r\n".into(), HP_CAPS.as_bytes().to_vec())
                } else if request == "GET /eSCL/ScannerStatus" {
                    let xml = format!("<scan:ScannerStatus xmlns:scan=\"s\" xmlns:pwg=\"p\"><pwg:State>Idle</pwg:State><scan:AdfState>{adf_state}</scan:AdfState></scan:ScannerStatus>");
                    ("200 OK", String::new(), xml.into_bytes())
                } else if request == "POST /eSCL/ScanJobs" {
                    ("201 Created", format!("Location: http://127.0.0.1:{port}/eSCL/ScanJobs/job-1\r\n"), Vec::new())
                } else if request == "GET /eSCL/ScanJobs/job-1/NextDocument" {
                    if served < pages && waits < not_ready {
                        waits += 1;
                        ("503 Service Unavailable", String::new(), Vec::new())
                    } else if served < pages {
                        served += 1;
                        waits = 0;
                        let jpeg = crate::scanner::pdf::tests::sample_jpeg(2480, 3508, 3);
                        ("200 OK", "Content-Type: image/jpeg\r\n".into(), jpeg)
                    } else {
                        ("404 Not Found", String::new(), Vec::new())
                    }
                } else if request == "DELETE /eSCL/ScanJobs/job-1" {
                    ("200 OK", String::new(), Vec::new())
                } else {
                    ("404 Not Found", String::new(), Vec::new())
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&payload);
            }
        });
        FakeScanner { root: format!("http://127.0.0.1:{port}/eSCL"), requests }
    }

    fn settings(source: Source) -> Settings {
        Settings { source, mode: Mode::Color, resolution: 300, paper: Paper::A4 }
    }

    #[test]
    fn the_feeder_is_read_until_the_scanner_says_it_is_empty() {
        let fake = fake_scanner(3, "ScannerAdfLoaded", 1);
        let seen = Mutex::new(Vec::new());
        let scanned = Escl::new(&fake.root)
            .unwrap()
            .scan(settings(Source::Feeder), &|line| seen.lock().unwrap().push(line.to_string()), &AtomicBool::new(false))
            .unwrap();
        let Scanned::Pages(pages) = scanned else { panic!("expected pages") };
        assert_eq!(pages.len(), 3);
        assert_eq!(pages[0].dpi, 300);
        assert!(seen.lock().unwrap().iter().any(|l| l == "Scanning… 2 pages so far"));
        assert!(!seen.lock().unwrap().iter().any(|l| l.contains("4")), "never promises a page that is not there");

        let log = fake.requests.lock().unwrap();
        let post = log.iter().find(|r| r.starts_with("POST /eSCL/ScanJobs")).unwrap();
        assert!(post.contains("<pwg:InputSource>Feeder</pwg:InputSource>"));
        assert_eq!(log.last().unwrap().trim(), "DELETE /eSCL/ScanJobs/job-1", "job released");
    }

    #[test]
    fn the_glass_gives_exactly_one_page() {
        let fake = fake_scanner(5, "ScannerAdfEmpty", 0);
        let scanned = Escl::new(&fake.root).unwrap().scan(settings(Source::Flatbed), &|_| {}, &AtomicBool::new(false)).unwrap();
        let Scanned::Pages(pages) = scanned else { panic!("expected pages") };
        assert_eq!(pages.len(), 1);
        let asked = fake.requests.lock().unwrap().iter().filter(|r| r.contains("NextDocument")).count();
        assert_eq!(asked, 1, "never ask a flatbed for a second page");
    }

    #[test]
    fn an_empty_feeder_is_reported_before_a_job_is_started() {
        let fake = fake_scanner(0, "ScannerAdfEmpty", 0);
        let error = Escl::new(&fake.root).unwrap().scan(settings(Source::Feeder), &|_| {}, &AtomicBool::new(false)).unwrap_err();
        assert!(error.contains("feeder is empty"), "{error}");
        assert!(!fake.requests.lock().unwrap().iter().any(|r| r.starts_with("POST")));
    }

    /// Against a real eSCL server — AirSane in front of SANE's test driver
    /// is what this was checked with. Set MECLOUD_ESCL_TEST_URL to its root;
    /// MECLOUD_TEST_OUT, if set, receives the PDFs for an outside check.
    #[test]
    fn scans_from_a_real_escl_server() {
        let Ok(root) = std::env::var("MECLOUD_ESCL_TEST_URL") else {
            eprintln!("skipped: set MECLOUD_ESCL_TEST_URL to an eSCL root");
            return;
        };
        let scanner = Escl::new(&root).unwrap();
        let caps = scanner.capabilities().unwrap();
        let options = caps.options();
        assert!(options.source(Source::Flatbed).is_some(), "{options:?}");
        let out = std::env::var("MECLOUD_TEST_OUT").ok();
        for (source, mode) in [(Source::Flatbed, Mode::Color), (Source::Flatbed, Mode::Gray), (Source::Feeder, Mode::Color)] {
            let Some(caps) = options.source(source) else { continue };
            let settings = crate::scanner::fit(
                &options,
                Settings { source, mode, resolution: 150, paper: Paper::A4 },
            )
            .unwrap();
            assert!(caps.resolutions.contains(&settings.resolution));
            let scanned = scanner.scan(settings, &|_| {}, &AtomicBool::new(false)).unwrap();
            let Scanned::Pages(pages) = scanned else { panic!("expected JPEG pages") };
            assert!(!pages.is_empty());
            let pdf = crate::scanner::pdf::build(&pages, &Default::default()).unwrap();
            if let Some(dir) = &out {
                std::fs::write(format!("{dir}/escl-{source:?}-{mode:?}.pdf"), &pdf).unwrap();
            }
        }
    }

    #[test]
    fn cancelling_still_releases_the_job() {
        let fake = fake_scanner(3, "ScannerAdfLoaded", 0);
        let error = Escl::new(&fake.root).unwrap().scan(settings(Source::Feeder), &|_| {}, &AtomicBool::new(true)).unwrap_err();
        assert_eq!(error, "Cancelled.");
        assert!(fake.requests.lock().unwrap().iter().any(|r| r.starts_with("DELETE")));
    }
}
