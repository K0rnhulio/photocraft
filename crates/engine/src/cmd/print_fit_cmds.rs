//! Image › Fit to Print Size: lay artwork out on a print page in millimetres in one step.
//!
//! For flatbed UV printing an artwork is usually prepared at a fixed physical size with room
//! around it (for example 100 x 160 mm of artwork on a 120 x 180 mm page). By hand that is Trim,
//! Image Size and Canvas Size with millimetre arithmetic in between; this command does the three as
//! one history step so an agent (or an action) can't get the arithmetic wrong.

use photocraft_geom::Rect;
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::image_cmds::{canvas_size, crop_doc, flatten_whole, image_size};
use crate::{EngineError, Result, Session};

pub const ID: &str = "image.fitToPrint";

const MM_PER_INCH: f64 = 25.4;
/// The largest page side accepted, in millimetres (5 m: larger than any flatbed).
const MAX_MM: f64 = 5_000.0;

fn bad(msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: ID.into(), msg: msg.into() }
}

fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document open".into())
}

/// A millimetre length from `p[key]`, or `default` when absent.
fn mm(p: &Value, key: &str, default: Option<f64>) -> Result<f64> {
    match p.get(key) {
        None | Some(Value::Null) => default.ok_or_else(|| bad(format!("`{key}` (millimetres) is required"))),
        Some(v) => v
            .as_f64()
            .filter(|v| v.is_finite() && *v > 0.0 && *v <= MAX_MM)
            .ok_or_else(|| bad(format!("`{key}` must be a length in millimetres, more than 0 and at most {MAX_MM}"))),
    }
}

/// Which margin [`trim_bounds`] cuts away.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Trim {
    None,
    White,
    Transparent,
}

/// The bounds of the pixels that are artwork: not (near) white for [`Trim::White`] (transparent
/// pixels count as white, as they print as paper), not transparent for [`Trim::Transparent`].
/// `None` when nothing is artwork.
fn trim_bounds(px: &[[f32; 4]], width: usize, trim: Trim, tolerance: f32) -> Option<Rect> {
    let width = width.max(1);
    let is_art = |q: &[f32; 4]| match trim {
        Trim::None => true,
        Trim::Transparent => q[3] > 0.5 / 255.0,
        // Composited over white: what the pixel looks like on paper.
        Trim::White => (0..3).any(|c| 1.0 - (q[c] * q[3] + (1.0 - q[3])) > tolerance),
    };
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for (i, q) in px.iter().enumerate() {
        if is_art(q) {
            let (x, y) = (i % width, i / width);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
    }
    if x0 == usize::MAX {
        return None;
    }
    let c = |v: usize| i32::try_from(v).ok();
    Some(Rect::new(c(x0)?, c(y0)?, c(x1)?, c(y1)?))
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let page_w = mm(p, "width", None)?;
    let page_h = mm(p, "height", None)?;
    let art_w = mm(p, "artWidth", Some(page_w))?;
    let art_h = mm(p, "artHeight", Some(page_h))?;
    if art_w > page_w + 1e-9 || art_h > page_h + 1e-9 {
        return Err(bad(format!("the artwork box ({art_w} x {art_h} mm) must fit on the page ({page_w} x {page_h} mm)")));
    }
    let dpi = match p.get("resolution") {
        None | Some(Value::Null) => 300.0,
        Some(v) => v
            .as_f64()
            .filter(|d| d.is_finite() && (1.0..=10_000.0).contains(d))
            .ok_or_else(|| bad("`resolution` must be a number from 1 to 10000 (pixels per inch)"))?,
    };
    let trim = match p.get("trim").map(Value::as_str) {
        None | Some(Some("none")) => Trim::None,
        Some(Some("white")) => Trim::White,
        Some(Some("transparent")) => Trim::Transparent,
        _ => return Err(bad("`trim` must be none, white or transparent")),
    };
    let tolerance = match p.get("tolerance") {
        None | Some(Value::Null) => 10.0,
        Some(v) => v.as_f64().filter(|t| t.is_finite() && (0.0..=255.0).contains(t)).ok_or_else(|| bad("`tolerance` must be a number from 0 to 255"))?,
    } as f32
        / 255.0;
    let upscale = match p.get("upscale") {
        None | Some(Value::Null) => true,
        Some(v) => v.as_bool().ok_or_else(|| bad("`upscale` must be true or false"))?,
    };
    let resample = match p.get("resample") {
        None | Some(Value::Null) => "automatic",
        Some(Value::String(r)) => r.as_str(),
        Some(_) => return Err(bad("`resample` must be a string (see image.imageSize)")),
    };
    let background = match p.get("background") {
        None | Some(Value::Null) => "white",
        Some(Value::String(b)) => b.as_str(),
        Some(_) => return Err(bad("`background` must be a string (see image.canvasSize extensionColor)")),
    };

    // Work out the trim before touching the document, so a refusal changes nothing.
    let d = s.active().ok_or(EngineError::NoDocument)?;
    let trimmed = if trim == Trim::None {
        None
    } else {
        let buf = flatten_whole(&d.doc, ID)?;
        let r = trim_bounds(&buf.px, d.doc.size.width as usize, trim, tolerance).ok_or_else(|| bad("the image is empty: nothing is left after trimming"))?;
        let canvas = d.doc.bounds();
        let r = Rect::new(r.x0 + canvas.x0, r.y0 + canvas.y0, r.x1 + canvas.x0, r.y1 + canvas.y0);
        (r != canvas).then_some(r)
    };
    let (src_w, src_h) = match trimmed {
        Some(r) => (f64::from(r.width()), f64::from(r.height())),
        None => (f64::from(d.doc.size.width), f64::from(d.doc.size.height)),
    };
    let px = |mm: f64| mm / MM_PER_INCH * dpi;
    let mut scale = (px(art_w) / src_w.max(1.0)).min(px(art_h) / src_h.max(1.0));
    if !upscale {
        scale = scale.min(1.0);
    }
    let (new_w, new_h) = ((src_w * scale).round().max(1.0), (src_h * scale).round().max(1.0));
    let (page_px_w, page_px_h) = (px(page_w).round().max(1.0), px(page_h).round().max(1.0));
    // A rounding pixel could otherwise poke past an artwork box as large as the page.
    let (new_w, new_h) = (new_w.min(page_px_w), new_h.min(page_px_h));

    crate::analysis_cmds::compound(s, "Fit to Print Size", |s| {
        if let Some(r) = trimmed {
            s.set_sampler_map(photocraft_geom::Affine::translate(-f64::from(r.x0), -f64::from(r.y0)));
            let extension = s.tools.background;
            s.edit("Trim", |doc, _| crop_doc(doc, ID, r, true, extension))?;
        }
        image_size(s, &json!({ "width": new_w, "height": new_h, "resolution": dpi, "resample": resample }))?;
        canvas_size(s, &json!({ "width": page_px_w, "height": page_px_h, "anchor": "center", "extensionColor": background }))
    })?;

    let d = s.active().ok_or(EngineError::NoDocument)?;
    let to_mm = |v: f64| (v / dpi * MM_PER_INCH * 10.0).round() / 10.0;
    Ok(json!({
        "width": d.doc.size.width,
        "height": d.doc.size.height,
        "resolution": dpi,
        "widthMm": to_mm(f64::from(d.doc.size.width)),
        "heightMm": to_mm(f64::from(d.doc.size.height)),
        "artwork": {
            "width": new_w,
            "height": new_h,
            "widthMm": to_mm(new_w),
            "heightMm": to_mm(new_h),
            "scale": (scale * 1000.0).round() / 1000.0,
        },
        "trimmed": trimmed.map(|r| json!({ "x": r.x0, "y": r.y0, "width": r.width(), "height": r.height() })),
    }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: ID,
        label: "Fit to Print Size…",
        menu: &[],
        shortcut: None,
        params: r##"{"width":mm,"height":mm (the page, the final print size),"artWidth":mm?,"artHeight":mm? (the box the artwork must fit inside, centred on the page; default the whole page),"resolution":ppi=300,"trim":"none|white|transparent"="none" (first cut away a white or transparent margin around the artwork),"tolerance":0..255=10 (how far from pure white still counts as white margin),"upscale":bool=true (false: never enlarge, only shrink to fit),"resample":"automatic|preserveDetails|bicubicSmoother|bicubicSharper|bicubic|bilinear|nearest|lanczos"="automatic","background":"white|transparent|background|#rrggbb"="white" (the new page area on the Background layer; other layers stay transparent there)} → {width,height,resolution,widthMm,heightMm,artwork:{width,height,widthMm,heightMm,scale},trimmed:{x,y,width,height}|null}: scales the artwork, keeping its proportions, to fit the artwork box at the resolution and centres it on a page of the given size; Trim, Image Size and Canvas Size in one undo step"##,
        enabled: has_doc,
        journal: true,
        run,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(w: u32, h: u32, dpi: f64) -> Session {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": w, "height": h, "resolution": dpi, "background": "white"})).unwrap();
        s
    }

    fn size(s: &Session) -> (u32, u32, f32) {
        let d = s.active().unwrap();
        (d.doc.size.width, d.doc.size.height, d.doc.resolution_dpi)
    }

    #[test]
    fn fits_artwork_inside_the_box_and_centres_it_on_the_page() {
        // Portrait artwork, 1000 x 1600 px at 72 ppi, onto 100 x 160 mm inside a 120 x 180 mm page.
        let mut s = session(1000, 1600, 72.0);
        let before = s.active().unwrap().history.entries();
        let r = s.execute(ID, json!({"width": 120, "height": 180, "artWidth": 100, "artHeight": 160})).unwrap();
        // 120 mm and 180 mm at 300 ppi.
        assert_eq!(size(&s), (1417, 2126, 300.0));
        assert_eq!(r["artwork"]["width"], json!(1181.0));
        assert_eq!(r["artwork"]["height"], json!(1890.0));
        assert_eq!(r["widthMm"], json!(120.0));
        assert_eq!(r["heightMm"], json!(180.0));
        assert_eq!(r["artwork"]["heightMm"], json!(160.0));
        assert_eq!(r["trimmed"], Value::Null);
        // One history step, and one undo takes it all back.
        let after = s.active().unwrap().history.entries();
        assert_eq!(after.len(), before.len() + 1);
        assert_eq!(after.last().map(String::as_str), Some("Fit to Print Size"));
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(size(&s), (1000, 1600, 72.0));
    }

    #[test]
    fn keeps_the_proportions_when_the_box_has_another_shape() {
        // A square image in a 100 x 160 mm box is limited by the width.
        let mut s = session(500, 500, 300.0);
        let r = s.execute(ID, json!({"width": 120, "height": 180, "artWidth": 100, "artHeight": 160})).unwrap();
        assert_eq!(r["artwork"]["width"], json!(1181.0));
        assert_eq!(r["artwork"]["height"], json!(1181.0));
    }

    #[test]
    fn trims_a_white_margin_before_fitting() {
        // A dark 40 x 80 px figure in the middle of a white 400 x 400 px image.
        let mut s = session(400, 400, 300.0);
        s.execute("select.rect", json!({"x": 180, "y": 160, "width": 40, "height": 80})).unwrap();
        s.execute("edit.fill", json!({"contents": "color", "color": "#202020"})).unwrap();
        s.execute("select.deselect", json!({})).unwrap();
        let r = s.execute(ID, json!({"width": 120, "height": 180, "artWidth": 100, "artHeight": 160, "trim": "white"})).unwrap();
        assert_eq!(r["trimmed"], json!({"x": 180, "y": 160, "width": 40, "height": 80}));
        // 40 x 80 px is 1:2, so the 160 mm height limits it: 945 x 1890 px.
        assert_eq!(r["artwork"]["height"], json!(1890.0));
        assert_eq!(r["artwork"]["width"], json!(945.0));
        assert_eq!(size(&s), (1417, 2126, 300.0));
    }

    #[test]
    fn upscale_false_only_shrinks() {
        let mut s = session(200, 300, 300.0);
        let r = s.execute(ID, json!({"width": 120, "height": 180, "upscale": false})).unwrap();
        assert_eq!(r["artwork"]["width"], json!(200.0));
        assert_eq!(size(&s), (1417, 2126, 300.0));
    }

    #[test]
    fn refuses_bad_params_without_changing_the_document() {
        let mut s = session(100, 100, 72.0);
        let history = s.active().unwrap().history.entries();
        for p in [
            json!({}),
            json!({"width": 120}),
            json!({"width": 0, "height": 10}),
            json!({"width": -5, "height": 10}),
            json!({"width": "120", "height": 180}),
            json!({"width": 120, "height": 180, "artWidth": 130}),
            json!({"width": 120, "height": 180, "resolution": 0}),
            json!({"width": 120, "height": 180, "trim": "black"}),
            json!({"width": 120, "height": 180, "tolerance": 300}),
            json!({"width": 1e300, "height": 180}),
        ] {
            let err = s.execute(ID, p.clone()).unwrap_err();
            assert!(matches!(err, EngineError::BadParams { .. }), "{p}: {err}");
        }
        // An all-white image has nothing left to trim to.
        assert!(s.execute(ID, json!({"width": 120, "height": 180, "trim": "white"})).is_err());
        assert_eq!(size(&s), (100, 100, 72.0));
        assert_eq!(s.active().unwrap().history.entries(), history);
    }
}
