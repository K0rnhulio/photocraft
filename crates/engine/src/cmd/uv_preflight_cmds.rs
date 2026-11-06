//! File › UV Print Preflight…: checks a document before it goes to a UV printer's RIP.
//!
//! Flatbed UV printers driven by MainTop (and similar RIPs) print white and varnish from spot
//! colour channels found by name: `W1` and `W2` are white, `W3` and `W4` are varnish. The RIP
//! matches the names exactly, so a channel named `w1`, `W 1` or `W1 copy` prints nothing. The
//! check reports the print size and lists every extra channel with its role and how much of the
//! image it inks, then raises:
//!
//! - errors: two channels with the same W name, a document with no pixels;
//! - warnings: resolution below the minimum, a near-miss W name, a W channel that is an alpha
//!   channel instead of a spot channel, an empty W channel, no white channel, another spot
//!   channel the RIP won't map, a colour mode or bit depth the RIP may not read;
//! - notes: no varnish channel, a W channel that covers the whole image, other alpha channels.
//!
//! Ink limits are set in the RIP, so they aren't checked here. The command is a query: it never
//! changes the document and isn't an undo step.

use photocraft_color::{ColorMode, SampleType};
use photocraft_doc::{AlphaChannel, Document};
use photocraft_geom::Rect;
use photocraft_raster::Surface;
use rayon::prelude::*;
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

/// The resolution below which the check warns, unless `minDpi` says otherwise.
pub const DEFAULT_MIN_DPI: f64 = 300.0;

/// A channel value above this counts as ink (half of one 8-bit step).
const INK: f32 = 0.5 / 255.0;

/// Rows read at once when measuring a channel.
const BAND: u32 = 256;

/// What the RIP does with a channel named exactly `W1`–`W4`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    White,
    Varnish,
}

impl Role {
    fn key(self) -> &'static str {
        match self {
            Role::White => "white",
            Role::Varnish => "varnish",
        }
    }
}

/// The role of an exact RIP channel name.
pub fn role(name: &str) -> Option<Role> {
    match name {
        "W1" | "W2" => Some(Role::White),
        "W3" | "W4" => Some(Role::Varnish),
        _ => None,
    }
}

/// The RIP name `name` looks like but isn't: `w1`, `W 1`, `W_2`, `W3 copy` → `W1`, `W2`, `W3`.
pub fn near_miss(name: &str) -> Option<&'static str> {
    if role(name).is_some() {
        return None;
    }
    let mut chars = name.trim().chars().filter(|c| !matches!(c, ' ' | '_' | '-' | '.'));
    if !matches!(chars.next(), Some('W' | 'w')) {
        return None;
    }
    let target = match chars.next() {
        Some('1') => "W1",
        Some('2') => "W2",
        Some('3') => "W3",
        Some('4') => "W4",
        _ => return None,
    };
    // `W10` is another name, not a mistyped `W1`.
    if chars.next().is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(target)
}

/// How much of the image a channel inks.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Coverage {
    /// Pixels with any ink, as a fraction of the image (0..1).
    pub inked: f64,
    /// Mean ink over the image (0..1).
    pub mean: f64,
}

impl Coverage {
    pub fn is_empty(&self) -> bool {
        self.inked <= 0.0
    }
    pub fn is_full(&self) -> bool {
        self.inked >= 1.0
    }
}

/// Measures `surface` over `area`, band by band so a large image never needs one big buffer.
pub fn coverage(surface: &Surface, area: Rect) -> Coverage {
    let (w, h) = (area.width(), area.height());
    let total = f64::from(w) * f64::from(h);
    if total <= 0.0 {
        return Coverage::default();
    }
    let ch = surface.channels().max(1);
    let fallback = surface.default_pixel().first().copied().unwrap_or(0.0).clamp(0.0, 1.0);
    let bands: Vec<Rect> = (0..h.div_ceil(BAND))
        .map(|b| {
            let y0 = area.y0.saturating_add((b * BAND) as i32);
            Rect::new(area.x0, y0, area.x1, y0.saturating_add(BAND as i32).min(area.y1))
        })
        .collect();
    let (inked, sum) = bands
        .par_iter()
        .map(|r| {
            let n = f64::from(r.width()) * f64::from(r.height());
            if !surface.has_tiles_in(*r) {
                // Untouched rows hold the surface's default value.
                return (if fallback > INK { n } else { 0.0 }, n * f64::from(fallback));
            }
            let mut data = Vec::new();
            surface.read_region_into(*r, &mut data);
            data.chunks_exact(ch).fold((0.0, 0.0), |(k, s), px| {
                let v = px.first().copied().unwrap_or(0.0);
                let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
                (if v > INK { k + 1.0 } else { k }, s + f64::from(v))
            })
        })
        .reduce(|| (0.0, 0.0), |a, b| (a.0 + b.0, a.1 + b.1));
    Coverage { inked: (inked / total).clamp(0.0, 1.0), mean: (sum / total).clamp(0.0, 1.0) }
}

fn percent(f: f64) -> String {
    let p = f * 100.0;
    if p > 0.0 && p < 0.1 { "<0.1%".into() } else { format!("{p:.1}%") }
}

fn check(level: &str, message: String) -> Value {
    json!({"level": level, "message": message})
}

/// One channel's row of the report, and the checks it raises.
fn channel_row(i: usize, c: &AlphaChannel, area: Rect, checks: &mut Vec<Value>) -> Value {
    let spot = c.spot.is_some();
    let role = role(&c.name);
    let cov = coverage(&c.surface, area);
    let name = &c.name;
    match (role, spot) {
        (Some(_), false) => checks.push(check(
            "warning",
            format!(
                "{name} is an alpha channel, not a spot colour channel, so the RIP may not print it. Double-click it in the Channels panel and choose Spot Color."
            ),
        )),
        (Some(_), true) if cov.is_empty() => checks.push(check("warning", format!("{name} is empty: it has no ink, so that layer won't print."))),
        (Some(r), true) if cov.is_full() => {
            checks.push(check("note", format!("{name} covers the whole image with {}. Check that's what you want.", r.key())))
        }
        _ => {}
    }
    if role.is_none() {
        if let Some(target) = near_miss(name) {
            checks.push(check("warning", format!("\"{name}\" looks like {target}, but the RIP needs the name exactly \"{target}\". Rename it.")));
        } else if spot {
            checks.push(check("warning", format!("Spot channel \"{name}\" isn't W1–W4, so the RIP won't use it for white or varnish.")));
        } else {
            checks.push(check("note", format!("Alpha channel \"{name}\" will be saved in the file too. Delete it if the RIP complains.")));
        }
    }
    json!({
        "index": i,
        "name": name,
        "kind": if spot { "spot" } else { "alpha" },
        "role": role.map(Role::key),
        "nearMiss": near_miss(name),
        "inked": cov.inked,
        "mean": cov.mean,
        "inkedText": percent(cov.inked),
        "empty": cov.is_empty(),
    })
}

/// The preflight report for `doc`: size, channels and checks (see the module docs).
pub fn report(doc: &Document, min_dpi: f64) -> Value {
    let (w, h) = (doc.size.width, doc.size.height);
    let dpi = f64::from(doc.resolution_dpi);
    let dpi = if dpi.is_finite() && dpi > 0.0 { dpi } else { 0.0 };
    let inches = |px: u32| if dpi > 0.0 { f64::from(px) / dpi } else { 0.0 };
    let (win, hin) = (inches(w), inches(h));
    let mut checks = Vec::new();

    if w == 0 || h == 0 {
        checks.push(check("error", "The image has no pixels.".into()));
    }
    if dpi <= 0.0 {
        checks.push(check("warning", "The image has no resolution set, so its print size is unknown. Set it in Image › Image Size.".into()));
    } else if dpi < min_dpi {
        checks.push(check(
            "warning",
            format!("Resolution is {dpi:.0} ppi, below {min_dpi:.0} ppi: the print may look soft. Image › Image Size shows the size at another resolution."),
        ));
    }
    match doc.mode {
        ColorMode::Rgb | ColorMode::Cmyk | ColorMode::Grayscale => {}
        m => checks.push(check("warning", format!("The image is in {} mode. The RIP expects RGB or CMYK (Image › Mode).", mode_name(m)))),
    }
    if doc.depth == SampleType::F32 {
        checks.push(check("warning", "The image is 32 bits per channel. Save 8 or 16 bits for the RIP (Image › Mode).".into()));
    }

    let area = doc.bounds();
    let channels: Vec<Value> = doc.channels.iter().enumerate().map(|(i, c)| channel_row(i, c, area, &mut checks)).collect();

    for target in ["W1", "W2", "W3", "W4"] {
        let n = doc.channels.iter().filter(|c| c.name == target).count();
        if n > 1 {
            checks.push(check("error", format!("{n} channels are named {target}. The RIP needs each name once; rename or delete the extra ones.")));
        }
    }
    let has = |r: Role| doc.channels.iter().any(|c| role(&c.name) == Some(r));
    if !has(Role::White) {
        checks.push(check(
            "warning",
            "No white channel (W1 or W2). Fine for white or light material; on dark or clear material colours need white under them.".into(),
        ));
    }
    if !has(Role::Varnish) {
        checks.push(check("note", "No varnish channel (W3 or W4).".into()));
    }

    // Errors first, then warnings, then notes.
    let rank = |c: &Value| match c["level"].as_str() {
        Some("error") => 0,
        Some("warning") => 1,
        _ => 2,
    };
    checks.sort_by_key(rank);
    let count = |l: &str| checks.iter().filter(|c| c["level"] == l).count();
    let (errors, warnings, notes) = (count("error"), count("warning"), count("note"));
    let status = if errors > 0 {
        "fix"
    } else if warnings > 0 {
        "check"
    } else {
        "ready"
    };
    json!({
        "document": doc.name,
        "status": status,
        "errors": errors,
        "warnings": warnings,
        "notes": notes,
        "size": {
            "width": w,
            "height": h,
            "dpi": dpi,
            "minDpi": min_dpi,
            "widthMm": win * 25.4,
            "heightMm": hin * 25.4,
            "widthIn": win,
            "heightIn": hin,
        },
        "mode": mode_name(doc.mode),
        "depth": doc.depth.bits(),
        "channels": channels,
        "checks": checks,
    })
}

/// The mode as Image › Mode names it.
fn mode_name(m: ColorMode) -> &'static str {
    match m {
        ColorMode::Bitmap => "Bitmap",
        ColorMode::Grayscale => "Grayscale",
        ColorMode::Indexed => "Indexed Color",
        ColorMode::Rgb => "RGB",
        ColorMode::Cmyk => "CMYK",
        ColorMode::Lab => "Lab",
        ColorMode::Multichannel => "Multichannel",
        ColorMode::Duotone => "Duotone",
    }
}

fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document open".into())
}

fn run(s: &Session, p: &Value) -> Result<Value> {
    let cmd = "file.uvPreflight";
    let min_dpi = match p.get("minDpi") {
        None | Some(Value::Null) => DEFAULT_MIN_DPI,
        Some(v) => v
            .as_f64()
            .filter(|d| d.is_finite() && (1.0..=100_000.0).contains(d))
            .ok_or_else(|| EngineError::BadParams { cmd: cmd.into(), msg: "`minDpi` must be a number from 1 to 100000".into() })?,
    };
    let st = s.active().ok_or(EngineError::NoDocument)?;
    Ok(report(&st.doc, min_dpi))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "file.uvPreflight",
        label: "UV Print Preflight…",
        menu: &["File"],
        shortcut: None,
        params: r#"{minDpi:1..100000=300} → {status:"ready|check|fix",errors,warnings,notes,size:{width,height,dpi,minDpi,widthMm,heightMm,widthIn,heightIn},mode,depth,channels:[{index,name,kind:"spot|alpha",role:"white|varnish"|null,nearMiss,inked,mean,inkedText,empty}],checks:[{level:"error|warning|note",message}]}: checks spot channels W1/W2 (white) and W3/W4 (varnish) and the print size before a UV print"#,
        enabled: has_doc,
        journal: false,
        run: |s, p| run(s, p),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_doc::AlphaChannel;

    fn session(w: u32, h: u32, dpi: f64) -> Session {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": w, "height": h, "resolution": dpi})).unwrap();
        s
    }

    /// Adds a channel named `name`, inked over `ink` (canvas pixels), spot or alpha.
    fn add(s: &mut Session, name: &str, spot: bool, ink: Option<Rect>) {
        s.edit("add channel", |doc, _| {
            let mut surface = Surface::new(crate::channel_cmds::channel_format(doc));
            if let Some(r) = ink {
                surface.fill_rect(r, &[1.0]);
            }
            let mut c = AlphaChannel::new(name, surface);
            if spot {
                c.spot = Some((photocraft_color::Color::rgb(1.0, 1.0, 1.0), 0.0));
            }
            doc.channels.push(c);
            Ok(())
        })
        .unwrap();
    }

    fn preflight(s: &mut Session) -> Value {
        s.execute("file.uvPreflight", json!({})).unwrap()
    }

    fn messages(r: &Value, level: &str) -> Vec<String> {
        r["checks"].as_array().unwrap().iter().filter(|c| c["level"] == level).map(|c| c["message"].as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn names_and_near_misses() {
        assert_eq!(role("W1"), Some(Role::White));
        assert_eq!(role("W2"), Some(Role::White));
        assert_eq!(role("W3"), Some(Role::Varnish));
        assert_eq!(role("W4"), Some(Role::Varnish));
        assert_eq!(role("w1"), None);
        for (name, target) in [("w1", "W1"), ("W 2", "W2"), ("W_3", "W3"), ("W4 copy", "W4"), (" W1 ", "W1"), ("w-2", "W2")] {
            assert_eq!(near_miss(name), Some(target), "{name}");
        }
        for name in ["W1", "W10", "W5", "White", "Spot Color 1", "", "W"] {
            assert_eq!(near_miss(name), None, "{name}");
        }
    }

    #[test]
    fn reports_size_in_pixels_mm_and_inches() {
        let mut s = session(3000, 1500, 300.0);
        let r = preflight(&mut s);
        let size = &r["size"];
        assert_eq!((size["width"].as_u64(), size["height"].as_u64()), (Some(3000), Some(1500)));
        assert_eq!(size["dpi"], 300.0);
        assert_eq!(r["mode"], "RGB");
        assert!((size["widthIn"].as_f64().unwrap() - 10.0).abs() < 1e-9);
        assert!((size["widthMm"].as_f64().unwrap() - 254.0).abs() < 1e-9);
        assert!((size["heightMm"].as_f64().unwrap() - 127.0).abs() < 1e-9);
    }

    #[test]
    fn a_complete_file_is_ready() {
        let mut s = session(40, 20, 600.0);
        add(&mut s, "W1", true, Some(Rect::new(0, 0, 20, 20)));
        add(&mut s, "W3", true, Some(Rect::new(0, 0, 10, 10)));
        let r = preflight(&mut s);
        assert_eq!(r["status"], "ready", "{r:#}");
        assert_eq!(r["channels"][0]["role"], "white");
        assert_eq!(r["channels"][1]["role"], "varnish");
        assert_eq!(r["channels"][0]["inked"], 0.5);
        assert_eq!(r["channels"][1]["inkedText"], "12.5%");
    }

    #[test]
    fn warns_about_low_resolution_and_honours_min_dpi() {
        let mut s = session(40, 20, 150.0);
        add(&mut s, "W1", true, Some(Rect::new(0, 0, 4, 4)));
        let r = preflight(&mut s);
        assert_eq!(r["status"], "check");
        assert!(messages(&r, "warning").iter().any(|m| m.contains("150 ppi")), "{r:#}");
        let r = s.execute("file.uvPreflight", json!({"minDpi": 100})).unwrap();
        assert!(!messages(&r, "warning").iter().any(|m| m.contains("ppi")), "{r:#}");
    }

    #[test]
    fn flags_missing_white_and_varnish() {
        let mut s = session(10, 10, 300.0);
        let r = preflight(&mut s);
        assert!(messages(&r, "warning").iter().any(|m| m.contains("No white channel")));
        assert!(messages(&r, "note").iter().any(|m| m.contains("No varnish channel")));
    }

    #[test]
    fn flags_wrong_names_alpha_channels_and_empty_channels() {
        let mut s = session(10, 10, 300.0);
        add(&mut s, "w1", true, Some(Rect::new(0, 0, 5, 5)));
        add(&mut s, "W2", false, Some(Rect::new(0, 0, 5, 5)));
        add(&mut s, "W3", true, None);
        add(&mut s, "Gold", true, Some(Rect::new(0, 0, 1, 1)));
        add(&mut s, "Alpha 1", false, None);
        let r = preflight(&mut s);
        let warnings = messages(&r, "warning");
        assert!(warnings.iter().any(|m| m.contains("\"w1\" looks like W1")), "{warnings:?}");
        assert!(warnings.iter().any(|m| m.starts_with("W2 is an alpha channel")), "{warnings:?}");
        assert!(warnings.iter().any(|m| m.starts_with("W3 is empty")), "{warnings:?}");
        assert!(warnings.iter().any(|m| m.contains("\"Gold\" isn't W1–W4")), "{warnings:?}");
        assert!(messages(&r, "note").iter().any(|m| m.contains("\"Alpha 1\"")));
        assert_eq!(r["channels"][0]["nearMiss"], "W1");
        assert_eq!(r["channels"][2]["empty"], true);
        assert_eq!(r["channels"][4]["kind"], "alpha");
    }

    #[test]
    fn duplicate_names_are_errors_and_come_first() {
        let mut s = session(10, 10, 300.0);
        add(&mut s, "W1", true, Some(Rect::new(0, 0, 5, 5)));
        add(&mut s, "W1", true, Some(Rect::new(5, 5, 10, 10)));
        let r = preflight(&mut s);
        assert_eq!(r["status"], "fix");
        assert_eq!(r["errors"], 1);
        assert_eq!(r["checks"][0]["level"], "error");
        assert!(r["checks"][0]["message"].as_str().unwrap().contains("2 channels are named W1"));
    }

    #[test]
    fn full_coverage_is_a_note() {
        let mut s = session(10, 10, 300.0);
        add(&mut s, "W1", true, Some(Rect::new(0, 0, 10, 10)));
        let r = preflight(&mut s);
        assert!(messages(&r, "note").iter().any(|m| m.contains("W1 covers the whole image")));
        assert_eq!(r["status"], "ready", "notes alone don't hold a file back: {r:#}");
    }

    #[test]
    fn coverage_spans_bands_and_untouched_rows() {
        // Taller than one band, inked only in the last rows: the untouched bands count as empty.
        let area = Rect::new(0, 0, 4, 600);
        let mut surface = Surface::new(photocraft_color::PixelFormat::new(ColorMode::Grayscale, SampleType::U8, false));
        surface.fill_rect(Rect::new(0, 540, 4, 600), &[1.0]);
        let c = coverage(&surface, area);
        assert!((c.inked - 0.1).abs() < 1e-9, "{c:?}");
        assert!((c.mean - 0.1).abs() < 1e-9, "{c:?}");
        assert_eq!(coverage(&surface, Rect::EMPTY), Coverage::default());
    }

    #[test]
    fn is_a_query() {
        let mut s = session(10, 10, 300.0);
        add(&mut s, "W1", true, Some(Rect::new(0, 0, 5, 5)));
        let steps = s.active().unwrap().history.past_len();
        let revision = s.active().unwrap().revision;
        preflight(&mut s);
        assert_eq!(s.active().unwrap().history.past_len(), steps);
        assert_eq!(s.active().unwrap().revision, revision);
    }

    #[test]
    fn bad_params_and_no_document_are_errors() {
        let mut s = session(10, 10, 300.0);
        for bad in [json!({"minDpi": "high"}), json!({"minDpi": 0}), json!({"minDpi": -5}), json!({"minDpi": 1e9})] {
            assert!(s.execute("file.uvPreflight", bad.clone()).is_err(), "{bad}");
        }
        let mut empty = Session::new();
        assert!(empty.execute("file.uvPreflight", json!({})).is_err());
    }
}
