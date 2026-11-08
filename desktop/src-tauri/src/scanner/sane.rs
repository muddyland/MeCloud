//! SANE, through `scanimage`.
//!
//! SANE is how Linux talks to scanners — USB ones in particular, which eSCL
//! cannot reach — and `scanimage` is its command-line front end, installed
//! with the drivers on every distribution (`sane-utils` on Debian and Ubuntu).
//! Running it rather than linking `libsane` keeps the client free of a C
//! dependency that would have to be present for the client to start at all,
//! and means a driver crashing takes down a child process, not MeCloud.
//!
//! Option names are standard (`--mode`, `--source`, `--resolution`, `-x`,
//! `-y`); their *values* are each driver's own. "Color" or "Colour", "ADF"
//! or "Automatic Document Feeder" — so `scanimage -A` is read for the values
//! this device offers, and each is matched to one of ours by what it says.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::pdf::{ImageKind, Page};
use super::{area_mm, offered_resolutions, Mode, Options, Scanned, Settings, Source, SourceOptions};

/// The program, overridable for tests and for unusual installs.
fn program() -> String {
    std::env::var("MECLOUD_SCANIMAGE").unwrap_or_else(|_| "scanimage".into())
}

/// Whether `scanimage` can be run at all.
pub fn available() -> bool {
    Command::new(program()).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok()
}

/// Run a command to completion, killing it after `limit`.
fn run(args: &[&str], limit: Duration) -> Result<(String, String, bool), String> {
    let mut child = Command::new(program())
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not run scanimage: {e}"))?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(o) = stdout.as_mut() {
            let _ = std::io::Read::read_to_string(o, &mut s);
        }
        s
    });
    let err = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(e) = stderr.as_mut() {
            let _ = std::io::Read::read_to_string(e, &mut s);
        }
        s
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            return Err("The scanner driver did not answer in time.".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    Ok((out.join().unwrap_or_default(), err.join().unwrap_or_default(), status.success()))
}

/// Parse `scanimage -L`: ``device `name' is a Vendor Model type``.
pub fn parse_list(output: &str) -> Vec<(String, String)> {
    output
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("device `")?;
            let split = rest.find("' is a ")?;
            let device = rest[..split].to_string();
            let description = rest[split + "' is a ".len()..].trim().to_string();
            Some((device, description))
        })
        .collect()
}

/// The SANE devices on this machine. Slow — SANE probes USB and the network —
/// so it is given a while, and run in parallel with mDNS by `discover`.
pub fn list() -> Result<Vec<(String, String)>, String> {
    let (stdout, stderr, ok) = run(&["-L"], Duration::from_secs(25))?;
    if !ok && stdout.trim().is_empty() {
        return Err(stderr.lines().last().unwrap_or("scanimage failed").to_string());
    }
    Ok(parse_list(&stdout))
}

/// One option line from `scanimage -A`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OptionLine {
    /// Discrete values, in the driver's own words.
    pub values: Vec<String>,
    /// `lo..hi`, for a range.
    pub range: Option<(f64, f64)>,
    pub inactive: bool,
}

/// The options this client uses, as `scanimage -A` describes them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeviceOptions {
    pub mode: Option<OptionLine>,
    pub source: Option<OptionLine>,
    pub resolution: Option<OptionLine>,
    pub x: Option<OptionLine>,
    pub y: Option<OptionLine>,
}

/// Read one option's values: `Gray|Color [Gray]`, `1..1200dpi (in steps of
/// 1) [50]`, `75|150|300dpi [75]`, `0..215.9mm [215.9]`.
fn parse_values(spec: &str) -> OptionLine {
    let mut line = OptionLine::default();
    // The current value is the last bracket; "[inactive]" means the option
    // exists but does not apply in the current state.
    let (body, current) = match spec.rfind(" [") {
        Some(i) => (&spec[..i], &spec[i + 2..]),
        None => (spec, ""),
    };
    line.inactive = current.starts_with("inactive");
    let body = match body.find(" (in steps") {
        Some(i) => &body[..i],
        None => body,
    };
    let strip_unit = |v: &str| v.trim().trim_end_matches("dpi").trim_end_matches("mm").trim_end_matches('%').to_string();
    if let Some((lo, hi)) = body.split_once("..") {
        if let (Ok(lo), Ok(hi)) = (strip_unit(lo).parse::<f64>(), strip_unit(hi).parse::<f64>()) {
            line.range = Some((lo, hi));
            return line;
        }
    }
    line.values = body.split('|').map(strip_unit).filter(|v| !v.is_empty()).collect();
    line
}

pub fn parse_options(output: &str) -> DeviceOptions {
    let mut options = DeviceOptions::default();
    for raw in output.lines() {
        let line = raw.trim_start();
        let (name, spec) = match line.split_once(' ') {
            Some(pair) => pair,
            None => continue,
        };
        let slot = match name {
            "--mode" => &mut options.mode,
            "--source" => &mut options.source,
            "--resolution" => &mut options.resolution,
            "-x" => &mut options.x,
            "-y" => &mut options.y,
            _ => continue,
        };
        if slot.is_none() {
            *slot = Some(parse_values(spec));
        }
    }
    options
}

/// Which of ours a driver's source value is.
pub fn classify_source(value: &str) -> Option<Source> {
    let v = value.to_ascii_lowercase();
    if v.contains("duplex") {
        return Some(Source::Duplex);
    }
    if v.contains("adf") || v.contains("feeder") || v.contains("automatic document") {
        return Some(Source::Feeder);
    }
    if v.contains("flatbed") || v.contains("platen") || v.contains("glass") || v == "normal" || v == "document table" {
        return Some(Source::Flatbed);
    }
    None
}

/// Which of ours a driver's mode value is.
pub fn classify_mode(value: &str) -> Option<Mode> {
    let v = value.to_ascii_lowercase();
    if v.contains("color") || v.contains("colour") {
        return Some(Mode::Color);
    }
    if v.contains("gray") || v.contains("grey") {
        return Some(Mode::Gray);
    }
    if v.contains("lineart") || v.contains("binary") || v.contains("black") || v == "mono" || v == "monochrome" {
        return Some(Mode::BlackWhite);
    }
    None
}

impl DeviceOptions {
    /// The driver's value for one of our sources or modes, first match wins.
    fn source_value(&self, source: Source) -> Option<String> {
        self.source.as_ref()?.values.iter().find(|v| classify_source(v) == Some(source)).cloned()
    }

    fn mode_value(&self, mode: Mode) -> Option<String> {
        self.mode.as_ref()?.values.iter().find(|v| classify_mode(v) == Some(mode)).cloned()
    }

    pub fn to_options(&self) -> Options {
        let sources: Vec<Source> = match &self.source {
            // A scanner with no source option has one: its glass.
            None => vec![Source::Flatbed],
            Some(line) => {
                let mut found: Vec<Source> = line.values.iter().filter_map(|v| classify_source(v)).collect();
                found.sort();
                found.dedup();
                found
            }
        };
        let mut modes: Vec<Mode> = match &self.mode {
            None => vec![Mode::Color],
            Some(line) => line.values.iter().filter_map(|v| classify_mode(v)).collect(),
        };
        modes.sort();
        modes.dedup();
        let resolutions = match &self.resolution {
            None => vec![300],
            Some(line) => {
                let discrete: Vec<u32> = line.values.iter().filter_map(|v| v.parse::<f64>().ok()).map(|v| v as u32).collect();
                offered_resolutions(&discrete, line.range.map(|(lo, hi)| (lo as u32, hi as u32)))
            }
        };
        let max_mm = match (&self.x, &self.y) {
            (Some(x), Some(y)) => match (x.range, y.range) {
                (Some((_, w)), Some((_, h))) => Some((w, h)),
                _ => None,
            },
            _ => None,
        };
        Options {
            sources: sources
                .into_iter()
                .map(|source| SourceOptions { source, modes: modes.clone(), resolutions: resolutions.clone(), max_mm })
                .collect(),
        }
    }
}

fn read_options(device: &str) -> Result<DeviceOptions, String> {
    let (stdout, stderr, ok) = run(&["-d", device, "-A"], Duration::from_secs(30))?;
    if !ok {
        return Err(format!(
            "The scanner did not answer: {}",
            stderr.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("scanimage failed")
        ));
    }
    Ok(parse_options(&stdout))
}

pub fn options(device: &str) -> Result<Options, String> {
    Ok(read_options(device)?.to_options())
}

/// The `scanimage` arguments for a scan into `dir`.
pub fn scan_args(device: &str, options: &DeviceOptions, settings: Settings, dir: &Path) -> (Vec<String>, ImageKind) {
    let mut args = vec!["-d".to_string(), device.to_string()];
    if let Some(mode) = options.mode_value(settings.mode) {
        args.push("--mode".into());
        args.push(mode);
    }
    if let Some(source) = options.source_value(settings.source) {
        args.push("--source".into());
        args.push(source);
    }
    if options.resolution.is_some() {
        args.push("--resolution".into());
        args.push(settings.resolution.to_string());
    }
    let max = match (&options.x, &options.y) {
        (Some(x), Some(y)) => match (x.range, y.range) {
            (Some((_, w)), Some((_, h))) => Some((w, h)),
            _ => None,
        },
        _ => None,
    };
    if max.is_some() {
        if let Some((w, h)) = area_mm(settings.paper, max) {
            args.extend(["-l".into(), "0".into(), "-t".into(), "0".into()]);
            args.extend(["-x".into(), format!("{w:.1}"), "-y".into(), format!("{h:.1}")]);
        }
    }
    // JPEG for colour and grey: a 300 dpi colour page is ~1 MB rather than
    // ~25 MB. PNG for black and white, where JPEG cannot hold one bit per
    // pixel and PNG is tiny.
    let kind = if settings.mode == Mode::BlackWhite { ImageKind::Png } else { ImageKind::Jpeg };
    let ext = if kind == ImageKind::Png { "png" } else { "jpg" };
    args.push(format!("--format={}", if kind == ImageKind::Png { "png" } else { "jpeg" }));
    if settings.source == Source::Flatbed {
        args.push("-o".into());
        args.push(dir.join(format!("page-0001.{ext}")).display().to_string());
    } else {
        // Batch mode reads the feeder until it is empty.
        args.push(format!("--batch={}", dir.join(format!("page-%04d.{ext}")).display()));
    }
    (args, kind)
}

/// A private scratch directory for one scan's pages.
fn scratch_dir() -> Result<PathBuf, String> {
    let base = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("mecloud").join("scans");
    std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700));
    }
    let nonce: u64 = rand_u64();
    let dir = base.join(format!("sane-{}-{nonce:x}", std::process::id()));
    std::fs::create_dir(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn rand_u64() -> u64 {
    let mut bytes = [0u8; 8];
    let _ = getrandom::fill(&mut bytes);
    u64::from_le_bytes(bytes)
}

struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// How long one scan may take, all pages included.
const SCAN_LIMIT: Duration = Duration::from_secs(30 * 60);

pub fn scan(device: &str, settings: Settings, progress: &dyn Fn(&str), cancel: &AtomicBool) -> Result<Scanned, String> {
    let options = read_options(device)?;
    let dir = scratch_dir()?;
    let _cleanup = Cleanup(dir.clone());
    let (args, kind) = scan_args(device, &options, settings, &dir);

    progress("Starting the scanner…");
    let mut child = Command::new(program())
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not run scanimage: {e}"))?;

    // Batch mode reports "Scanning page N" on stderr; that is the progress.
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let latest = Arc::new(Mutex::new(None::<String>));
    let stderr = child.stderr.take();
    let (sink, newest) = (lines.clone(), latest.clone());
    let reader = std::thread::spawn(move || {
        if let Some(err) = stderr {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                if let Some(n) = line.strip_prefix("Scanning page ") {
                    *newest.lock().unwrap() = Some(format!("Scanning page {}…", n.trim()));
                }
                sink.lock().unwrap().push(line);
            }
        }
    });

    progress(if settings.source == Source::Flatbed { "Scanning…" } else { "Scanning page 1…" });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if cancel.load(Ordering::Relaxed) || started.elapsed() > SCAN_LIMIT {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(if cancel.load(Ordering::Relaxed) { "Cancelled.".into() } else { "The scan took too long.".into() });
        }
        if let Some(line) = latest.lock().unwrap().take() {
            progress(&line);
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let _ = reader.join();

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("page-")))
        .collect();
    files.sort();

    let log = lines.lock().unwrap().join("\n");
    // Batch mode ends with the feeder running dry, which scanimage reports
    // as an error; pages on disk are what says whether it worked.
    let pages: Vec<Page> = files
        .iter()
        .filter_map(|path| std::fs::read(path).ok())
        .filter(|bytes| !bytes.is_empty())
        .map(|bytes| Page { kind, bytes, dpi: settings.resolution })
        .collect();
    if pages.is_empty() {
        let lower = log.to_ascii_lowercase();
        if lower.contains("out of documents") || lower.contains("no docs") || lower.contains("feeder") && lower.contains("empty") {
            return Err("The document feeder is empty. Put the pages in and try again.".into());
        }
        let last = log.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("no pages were produced");
        return Err(if status.success() { "The scanner finished without sending a page.".into() } else { format!("The scan failed: {last}") });
    }
    Ok(Scanned::Pages(pages))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::Paper;

    const TEST_DEVICE: &str = "All options specific to device `test:0':
  Scan Mode:
    --mode Gray|Color [Gray]
        Selects the scan mode (e.g., lineart, monochrome, or color).
    --depth 1|8|16 [8]
    --resolution 1..1200dpi (in steps of 1) [50]
        Sets the resolution of the scanned image.
    --source Flatbed|Automatic Document Feeder [Flatbed]
  Geometry:
    -l 0..200mm (in steps of 1) [0]
    -t 0..200mm (in steps of 1) [0]
    -x 0..200mm (in steps of 1) [80]
    -y 0..200mm (in steps of 1) [100]
";

    const EPSON: &str = "
    --mode Lineart|Gray|Color [Lineart]
    --resolution 75|150|300|600|1200dpi [75]
    --source Flatbed|ADF Front|ADF Duplex [Flatbed]
    -x 0..215.9mm [215.9]
    -y 0..297.18mm [297.18]
";

    #[test]
    fn lists_devices_including_names_with_spaces() {
        let out = "device `test:0' is a Noname frontend-tester virtual device\n\
                   device `airscan:e0:HP LaserJet MFP' is a eSCL HP LaserJet MFP ip=10.0.0.5\n\
                   \nNo scanners were identified.\n";
        let devices = parse_list(out);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[1].0, "airscan:e0:HP LaserJet MFP");
        assert_eq!(devices[1].1, "eSCL HP LaserJet MFP ip=10.0.0.5");
    }

    #[test]
    fn reads_option_values_ranges_and_units() {
        let options = parse_options(TEST_DEVICE);
        assert_eq!(options.mode.as_ref().unwrap().values, vec!["Gray", "Color"]);
        assert_eq!(options.resolution.as_ref().unwrap().range, Some((1.0, 1200.0)));
        assert_eq!(options.source.as_ref().unwrap().values, vec!["Flatbed", "Automatic Document Feeder"]);
        assert_eq!(options.x.as_ref().unwrap().range, Some((0.0, 200.0)));

        let epson = parse_options(EPSON);
        assert_eq!(epson.resolution.as_ref().unwrap().values, vec!["75", "150", "300", "600", "1200"]);
        assert_eq!(epson.y.as_ref().unwrap().range, Some((0.0, 297.18)));
        assert!(parse_values("Flatbed|ADF [inactive]").inactive);
    }

    #[test]
    fn driver_words_map_to_ours() {
        let options = parse_options(EPSON).to_options();
        let sources: Vec<Source> = options.sources.iter().map(|s| s.source).collect();
        assert_eq!(sources, vec![Source::Flatbed, Source::Feeder, Source::Duplex]);
        assert_eq!(options.sources[0].modes, vec![Mode::Color, Mode::Gray, Mode::BlackWhite]);
        assert_eq!(options.sources[0].resolutions, vec![150, 300, 600]);
        assert_eq!(classify_source("Document Table"), Some(Source::Flatbed));
        assert_eq!(classify_mode("24bit Color[Fast]"), Some(Mode::Color));
        assert_eq!(classify_mode("Halftone"), None);
    }

    #[test]
    fn a_feeder_scan_uses_batch_mode_and_the_drivers_own_words() {
        let options = parse_options(EPSON);
        let settings = Settings { source: Source::Duplex, mode: Mode::BlackWhite, resolution: 300, paper: Paper::Legal };
        let (args, kind) = scan_args("epson2:libusb:001:004", &options, settings, Path::new("/x"));
        assert_eq!(kind, ImageKind::Png);
        let joined = args.join(" ");
        assert!(joined.contains("--mode Lineart"), "{joined}");
        assert!(joined.contains("--source ADF Duplex"));
        assert!(joined.contains("-x 215.9 -y 297.2"), "legal paper cut to the glass: {joined}");
        assert!(joined.ends_with("--format=png --batch=/x/page-%04d.png"));

        let flatbed = Settings { source: Source::Flatbed, mode: Mode::Color, resolution: 150, paper: Paper::A4 };
        let (args, kind) = scan_args("test:0", &parse_options(TEST_DEVICE), flatbed, Path::new("/x"));
        assert_eq!(kind, ImageKind::Jpeg);
        assert!(args.join(" ").ends_with("--format=jpeg -o /x/page-0001.jpg"));
    }

    /// Against the real `scanimage` and SANE's own test driver, when the
    /// machine has them — the build image does, a developer laptop may not.
    /// Enabled with MECLOUD_SANE_TEST=1 and SANE_CONFIG_DIR pointing at a
    /// dll.conf that loads `test`.
    #[test]
    fn scans_with_the_sane_test_driver() {
        if std::env::var("MECLOUD_SANE_TEST").as_deref() != Ok("1") {
            eprintln!("skipped: set MECLOUD_SANE_TEST=1 with the SANE test backend enabled");
            return;
        }
        let devices = list().unwrap();
        assert!(devices.iter().any(|(d, _)| d == "test:0"), "{devices:?}");
        let options = options("test:0").unwrap();
        assert!(options.source(Source::Feeder).is_some());

        let flatbed = Settings { source: Source::Flatbed, mode: Mode::Color, resolution: 75, paper: Paper::A4 };
        let Scanned::Pages(pages) = scan("test:0", flatbed, &|_| {}, &AtomicBool::new(false)).unwrap() else { panic!() };
        assert_eq!(pages.len(), 1);
        let info = crate::scanner::pdf::jpeg_info(&pages[0].bytes).unwrap();
        // The test driver's glass is 200 mm square: A4 is cut to it.
        let expected = 200.0f64 / 25.4 * 75.0; // 590.6; drivers round either way
        assert!((info.width as f64 - expected).abs() < 2.0, "width {}", info.width);

        // Its feeder runs dry after 10 sheets.
        let feeder = Settings { source: Source::Feeder, mode: Mode::Gray, resolution: 50, paper: Paper::A4 };
        let seen = Mutex::new(Vec::new());
        let Scanned::Pages(pages) =
            scan("test:0", feeder, &|l| seen.lock().unwrap().push(l.to_string()), &AtomicBool::new(false)).unwrap()
        else {
            panic!()
        };
        assert_eq!(pages.len(), 10);
        let pdf = crate::scanner::pdf::build(&pages, &Default::default()).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
    }
}
