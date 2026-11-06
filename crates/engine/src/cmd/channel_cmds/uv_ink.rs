//! Channels › UV Ink Channel…: a white underbase or varnish spot channel for UV printing.

use super::manage::color_value;
use super::planes::{channel_format, plane_surface};
use super::refs::bad;
use super::view::touch;
use super::{AlphaChannel, Color, Document, EngineError, Rect, Result, Session, Value, json, sel};

/// The UV ink channels a flatbed UV printer's RIP maps to its extra heads by name. MainTop takes
/// W1 and W2 as white and W3 and W4 as varnish; each gets its own one-click preset.
pub const UV_INKS: [(&str, &str); 4] = [("W1", "White"), ("W2", "White"), ("W3", "Varnish"), ("W4", "Varnish")];

/// The UV Ink Channel's name when none is given: the first white.
pub const UNDERBASE_NAME: &str = "W1";

/// The on-screen ink of a new UV channel: white for white, a pale yellow for varnish so the two
/// tell apart in the Channels panel (the printer goes by the name, never this colour).
fn uv_ink_color(name: &str) -> Color {
    let varnish = UV_INKS.iter().any(|(n, kind)| *kind == "Varnish" && n.eq_ignore_ascii_case(name)) || name.to_ascii_lowercase().contains("varnish");
    if varnish { Color::rgb(1.0, 0.9, 0.5) } else { Color::rgb(1.0, 1.0, 1.0) }
}

/// The widest choke a UV Ink Channel takes, in pixels.
pub const MAX_UNDERBASE_CHOKE: f64 = 100.0;

/// The largest canvas a UV Ink Channel works on: the choke keeps a full-canvas mask and distance
/// map, so a bigger one is refused rather than aborting on allocation (as Select › Modify does).
const MAX_UNDERBASE_PIXELS: u64 = 1 << 30;

/// Channels › UV Ink Channel…: a white underbase or varnish spot channel for UV printing in one
/// step (by default W1, the first white). Its
/// coverage is the selection, or with no selection everything that isn't transparent in the
/// visible layers (the Background left out, since it covers the whole canvas). `choke` shrinks it
/// by that many pixels so white doesn't peek out around the colour. A spot channel already
/// named `name` is updated in place (keeping its ink), so running it again after an edit
/// replaces the old white instead of stacking a second channel.
///
/// With `hardEdges` (the default) a pixel gets full ink when it is at least half selected (or
/// half opaque) and none otherwise. A RIP reads any gray as partial ink, so a feathered or
/// fuzzy selection (Color Range, a soft brush in Quick Mask) would otherwise lay a faint coat
/// of white over the whole background.
pub(super) fn uv_ink_channel(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "channel.uvInk";
    let name = match p.get("name") {
        None | Some(Value::Null) => UNDERBASE_NAME.to_string(),
        Some(Value::String(n)) if !n.trim().is_empty() => n.trim().to_string(),
        Some(_) => return Err(bad(cmd, "`name` must be a non-empty string")),
    };
    let source = match p.get("source").map(Value::as_str) {
        None | Some(Some("auto")) => "auto",
        Some(Some("selection")) => "selection",
        Some(Some("artwork")) => "artwork",
        Some(Some("image")) => "image",
        _ => return Err(bad(cmd, "`source` must be \"auto\", \"selection\", \"artwork\" or \"image\"")),
    };
    let choke = match p.get("choke") {
        None | Some(Value::Null) => 0.0,
        Some(v) => v
            .as_f64()
            .filter(|c| (0.0..=MAX_UNDERBASE_CHOKE).contains(c))
            .ok_or_else(|| bad(cmd, format!("`choke` must be a number of pixels in 0..{MAX_UNDERBASE_CHOKE}")))?,
    };
    let hard = match p.get("hardEdges") {
        None | Some(Value::Null) => true,
        Some(v) => v.as_bool().ok_or_else(|| bad(cmd, "`hardEdges` must be true or false"))?,
    };
    let color = color_value(cmd, p, "color")?;
    let solidity = match p.get("solidity") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_f64().filter(|v| (0.0..=100.0).contains(v)).ok_or_else(|| bad(cmd, "`solidity` must be a number in 0..100"))? as f32 / 100.0),
    };
    let doc = &s.active().ok_or(EngineError::NoDocument)?.doc;
    let area = doc.bounds();
    if area.size().area() > MAX_UNDERBASE_PIXELS {
        return Err(bad(cmd, "the image is too large for a UV ink channel"));
    }
    let use_selection = match source {
        "image" => false,
        "selection" => {
            if doc.selection.is_none() {
                return Err(bad(cmd, "make a selection first (or use source \"artwork\")"));
            }
            true
        }
        "artwork" => false,
        _ => doc.selection.is_some(),
    };
    let mut mask = if source == "image" {
        image_mask(cmd, p, area)?
    } else if use_selection {
        sel::mask_from_surface(doc.selection.as_ref(), area)
    } else {
        // The artwork's own shape: the merged alpha of the visible layers without the Background.
        let mut art = Document::clone(doc);
        if let Some(bg) = art.background_mut() {
            bg.visible = false;
        }
        photocraft_compose::render(&art, area).px.iter().map(|p| p[3].clamp(0.0, 1.0)).collect()
    };
    if choke > 0.0 {
        mask = sel::contract(&mask, area.width() as usize, area.height() as usize, choke as f32);
    }
    if hard {
        for v in &mut mask {
            *v = if *v >= 0.5 { 1.0 } else { 0.0 };
        }
    }
    if mask.iter().all(|v| *v <= 0.0) {
        let why = match source {
            "image" => "the image has no ink in it: check `ink` (which colour of the image means ink)",
            _ if use_selection => "the selection leaves nothing to put ink under",
            _ => "there is no artwork to put ink under: every layer is transparent",
        };
        return Err(bad(cmd, if choke > 0.0 { format!("{why} after the choke") } else { why.to_string() }));
    }
    let (i, updated) = s.edit("UV Ink Channel", |doc, _| {
        let surface = plane_surface(&mask, area, channel_format(doc));
        if let Some((i, ch)) = doc.channels.iter_mut().enumerate().find(|(_, c)| c.spot.is_some() && c.name.eq_ignore_ascii_case(&name)) {
            ch.surface = surface;
            let (ink, sol) = ch.spot.unwrap_or((uv_ink_color(&name), 0.0));
            ch.spot = Some((color.unwrap_or(ink), solidity.unwrap_or(sol)));
            return Ok((i, true));
        }
        if doc.channels.iter().any(|c| c.name.eq_ignore_ascii_case(&name)) {
            return Err(bad(cmd, format!("\"{name}\" is an alpha channel, not a spot channel: rename it or pick another name")));
        }
        let ink = (color.unwrap_or_else(|| uv_ink_color(&name)), solidity.unwrap_or(0.0));
        doc.channels.push(AlphaChannel { spot: Some(ink), ..AlphaChannel::new(name.clone(), surface) });
        Ok((doc.channels.len() - 1, false))
    })?;
    if let Some(st) = s.active_mut() {
        st.channel_view.set_alpha_visible(i, true);
        touch(st);
    }
    let source = match source {
        "image" => "image",
        _ if use_selection => "selection",
        _ => "artwork",
    };
    Ok(json!({ "channel": i, "name": name, "updated": updated, "source": source }))
}

/// The largest mask image `channel.uvInk` decodes, in pixels (an 8-bit RGBA copy is held).
const MAX_INK_IMAGE_PIXELS: u64 = 1 << 28;

/// The ink plane (0..1 per canvas pixel) of the mask image in `p["image"]` (base64 file bytes, any
/// format PhotoCraft opens): its white (or with `"ink": "black"` its black) is ink, and transparent
/// pixels are none. It must be the canvas's size unless `stretch` scales it to fit.
fn image_mask(cmd: &str, p: &Value, area: Rect) -> Result<Vec<f32>> {
    let data = p.get("image").and_then(Value::as_str).ok_or_else(|| bad(cmd, "source \"image\" needs `image`: the image file's bytes as base64"))?;
    let bytes = crate::plugin_cmds::b64_decode(data).ok_or_else(|| bad(cmd, "`image` is not valid base64"))?;
    let white = match p.get("ink").map(Value::as_str) {
        None | Some(Some("white")) => true,
        Some(Some("black")) => false,
        _ => return Err(bad(cmd, "`ink` must be \"white\" or \"black\" (the colour of the image that means ink)")),
    };
    let stretch = match p.get("stretch") {
        None | Some(Value::Null) => false,
        Some(v) => v.as_bool().ok_or_else(|| bad(cmd, "`stretch` must be true or false"))?,
    };
    let img = photocraft_codecs::decode(&bytes).map_err(|e| bad(cmd, format!("`image` could not be read: {e}")))?;
    let (iw, ih) = (img.width(), img.height());
    if u64::from(iw) * u64::from(ih) > MAX_INK_IMAGE_PIXELS {
        return Err(bad(cmd, format!("the image ({iw} x {ih} px) is too large")));
    }
    let (w, h) = (area.width(), area.height());
    if (iw, ih) != (w, h) && !stretch {
        return Err(bad(
            cmd,
            format!("the image is {iw} x {ih} px but the document is {w} x {h} px: use an image of the document's size, or pass \"stretch\": true"),
        ));
    }
    let px = img.to_rgba8();
    let ink = |i: usize| -> f32 {
        let Some(q) = px.get(i * 4..i * 4 + 4) else { return 0.0 };
        let lum = (0.2126 * f32::from(q[0]) + 0.7152 * f32::from(q[1]) + 0.0722 * f32::from(q[2])) / 255.0;
        let a = f32::from(q[3]) / 255.0;
        (if white { lum } else { 1.0 - lum }) * a
    };
    let (iw, ih) = (iw.max(1) as usize, ih.max(1) as usize);
    let (w, h) = (w as usize, h as usize);
    let mut mask = Vec::with_capacity(w.saturating_mul(h));
    for y in 0..h {
        // Nearest pixel: a mask's edges stay as hard as they were drawn.
        let sy = (y * ih / h.max(1)).min(ih - 1);
        for x in 0..w {
            let sx = (x * iw / w.max(1)).min(iw - 1);
            mask.push(ink(sy * iw + sx));
        }
    }
    Ok(mask)
}
