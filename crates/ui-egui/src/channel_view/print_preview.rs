//! The UV print preview's picture ([`photocraft_engine::print_preview_cmds`]): the material, then
//! white ink, colour ink and varnish stacked on it, the way a flatbed UV printer lays them down.
//!
//! Colour ink is modelled as a filter over what is under it: the composite over white is how
//! much light each pixel lets through, so it multiplies the material or white ink below.
//! White areas of the image print no ink and show the material. Varnish brightens toward a
//! soft diagonal reflection, deepens the colour under it a little, and stacked varnish channels
//! (W3 + W4) read as raised, with light from the top left catching their edges.

use egui::Color32;
use photocraft_doc::Document;
use photocraft_engine::print_preview_cmds::{InkRole, PrintPreview, Substrate, ink_role};
use photocraft_geom::Rect;

use super::{q, sample};

/// Plain white material, a touch darker than white ink so printed white shows on it.
const WHITE_MATERIAL: [f32; 3] = [0.93, 0.93, 0.92];
const BLACK_MATERIAL: [f32; 3] = [0.07, 0.07, 0.08];
/// The two squares of the checkerboard seen through clear material.
const CLEAR_LIGHT: [f32; 3] = [0.86, 0.88, 0.89];
const CLEAR_DARK: [f32; 3] = [0.70, 0.72, 0.74];
/// UV white ink is nearly, not fully, opaque: one pass lets a little of the material through.
const WHITE_INK: [f32; 3] = [0.99, 0.99, 0.98];
const WHITE_INK_OPACITY: f32 = 0.9;

/// The print preview of `r` (sampled every `factor` px) over `transmit`, the composite over white
/// sampled the same way, as opaque pixels.
pub(super) fn render(doc: &Document, pp: PrintPreview, r: Rect, factor: u32, transmit: &[[f32; 4]]) -> Vec<Color32> {
    let f = factor.max(1);
    let (w, h) = (r.width().div_ceil(f) as usize, r.height().div_ceil(f) as usize);
    let n = w * h;
    let mut whites = Vec::new();
    let mut varnishes = Vec::new();
    for ch in &doc.channels {
        match ink_role(&ch.name) {
            Some(InkRole::White) if pp.white => whites.push(ch),
            Some(InkRole::Varnish) if pp.varnish => varnishes.push(ch),
            _ => {}
        }
    }
    // Material, in document coordinates so a partial redraw lines up with the rest.
    let cell = (doc.size.width.max(doc.size.height) / 64).max(8) as i32;
    let mut out: Vec<[f32; 3]> = (0..n)
        .map(|i| {
            let (x, y) = (r.x0 + (i % w) as i32 * f as i32, r.y0 + (i / w) as i32 * f as i32);
            match pp.substrate {
                Substrate::White => WHITE_MATERIAL,
                Substrate::Black => BLACK_MATERIAL,
                Substrate::Color(c) => c,
                Substrate::Clear => {
                    if (x.div_euclid(cell) + y.div_euclid(cell)) % 2 == 0 {
                        CLEAR_LIGHT
                    } else {
                        CLEAR_DARK
                    }
                }
            }
        })
        .collect();
    for ch in whites {
        for (o, v) in out.iter_mut().zip(sample(&ch.surface, r, f)) {
            let a = v.clamp(0.0, 1.0) * WHITE_INK_OPACITY;
            for j in 0..3 {
                o[j] = WHITE_INK[j] * a + o[j] * (1.0 - a);
            }
        }
    }
    if pp.color {
        for (o, t) in out.iter_mut().zip(transmit) {
            for j in 0..3 {
                o[j] *= t[j].clamp(0.0, 1.0);
            }
        }
    }
    if !varnishes.is_empty() {
        varnish(doc, &varnishes, r, f, w, &mut out);
    }
    out.into_iter().map(|p| Color32::from_rgb(q(p[0]), q(p[1]), q(p[2]))).collect()
}

/// Varnish over `out` (`w` × `h` samples of `r`): gloss where it is, relief where it is stacked.
fn varnish(doc: &Document, chans: &[&photocraft_doc::AlphaChannel], r: Rect, f: u32, w: usize, out: &mut [[f32; 3]]) {
    // Heights one sample beyond `r` on each side (where the document goes on), so the relief
    // at the edge of a partial redraw matches the full picture.
    let fi = f as i32;
    let ext = Rect::new(r.x0 - fi, r.y0 - fi, r.x1 + fi, r.y1 + fi).intersect(&doc.bounds());
    let (ew, eh) = (ext.width().div_ceil(f) as usize, ext.height().div_ceil(f) as usize);
    let (ox, oy) = (((r.x0 - ext.x0) / fi) as usize, ((r.y0 - ext.y0) / fi) as usize);
    let mut height = vec![0.0f32; ew * eh];
    for ch in chans {
        for (hv, v) in height.iter_mut().zip(sample(&ch.surface, ext, f)) {
            *hv += v.clamp(0.0, 1.0);
        }
    }
    let at = |x: isize, y: isize| -> f32 {
        let x = x.clamp(0, ew as isize - 1) as usize;
        let y = y.clamp(0, eh as isize - 1) as usize;
        height.get(y * ew + x).copied().unwrap_or(0.0)
    };
    let diag = (doc.size.width + doc.size.height).max(1) as f32;
    for (i, o) in out.iter_mut().enumerate() {
        let (sx, sy) = ((i % w + ox) as isize, (i / w + oy) as isize);
        let hgt = at(sx, sy);
        // Light from the top left: edges facing it brighten, edges facing away darken.
        let relief = (at(sx - 1, sy - 1) - at(sx + 1, sy + 1)).clamp(-2.0, 2.0) * -0.18;
        let cover = hgt.min(1.0);
        if cover <= 0.0 && relief == 0.0 {
            continue;
        }
        // A broad diagonal reflection, as from a window or a lamp over a glossy surface.
        let (x, y) = ((r.x0 + (i % w) as i32 * fi) as f32, (r.y0 + (i / w) as i32 * fi) as f32);
        let t = (x + y) / diag - 0.35;
        let gloss = cover * (0.1 + 0.3 * (-(t * t) / (2.0 * 0.12 * 0.12)).exp());
        for c in o.iter_mut() {
            // Varnish makes colours a little deeper, like a wet surface.
            let deep = c.powf(1.0 + 0.2 * cover);
            *c = (deep + (1.0 - deep) * gloss + relief * cover.max(0.5)).clamp(0.0, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Look, render as view};
    use egui::Color32;
    use photocraft_engine::Session;
    use serde_json::json;

    /// A 16 × 8 document: left half red, right half white; channel `name` filled where `fill` is
    /// selected.
    fn doc_with(channels: &[(&str, [i32; 4])]) -> Session {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 16, "height": 8})).unwrap();
        s.execute("select.rect", json!({"x": 0, "y": 0, "width": 8, "height": 8})).unwrap();
        s.execute("edit.fill", json!({"color": "#ff0000"})).unwrap();
        for (name, [x, y, w, h]) in channels {
            s.execute("select.rect", json!({"x": x, "y": y, "width": w, "height": h})).unwrap();
            s.execute("channel.newSpot", json!({"name": name})).unwrap();
        }
        s.execute("select.deselect", json!({})).unwrap();
        s
    }

    fn px(s: &Session) -> Vec<Color32> {
        let st = s.active().unwrap();
        view(&st.doc, &st.channel_view, st.doc.bounds(), 1, Look::default(), st.active_layer).unwrap()
    }

    #[test]
    fn colour_needs_white_ink_on_black_material() {
        // White ink under the top half of the red area only.
        let mut s = doc_with(&[("W1", [0, 0, 16, 4])]);
        s.execute("view.printPreview", json!({"substrate": "black"})).unwrap();
        let p = px(&s);
        let over_white = p[2];
        let no_white = p[6 * 16 + 2];
        assert!(over_white.r() > 200 && over_white.g() < 40, "red reads red over white ink: {over_white:?}");
        assert!(no_white.r() < 40, "red on black material without white nearly vanishes: {no_white:?}");
        // The white half of the image prints no colour: white ink shows on top, black below.
        assert!(p[12].r() > 220 && p[12].g() > 220, "white ink: {:?}", p[12]);
        assert!(p[6 * 16 + 12].r() < 40, "bare black material: {:?}", p[6 * 16 + 12]);
        // Turning the white channel off in the preview drops it.
        s.execute("view.printPreview", json!({"white": false})).unwrap();
        assert!(px(&s)[2].r() < 40);
    }

    #[test]
    fn white_material_shows_the_image_and_clear_shows_a_checkerboard() {
        let mut s = doc_with(&[]);
        s.execute("view.printPreview", json!({"substrate": "white"})).unwrap();
        let p = px(&s);
        assert!(p[0].r() > 200 && p[0].g() < 30, "{:?}", p[0]);
        assert!(p[12].r() > 220 && p[12].r() == p[12].g(), "plain material: {:?}", p[12]);
        s.execute("view.printPreview", json!({"substrate": "clear"})).unwrap();
        let p = px(&s);
        // 8 px cells: the white half of the image is the darker square, unprinted.
        assert_eq!(p[8], p[7 * 16 + 15], "one cell");
        assert_eq!(p[8], Color32::from_rgb(179, 184, 189), "clear material's dark square");
        s.execute("view.printPreview", json!({"substrate": "#3366ff", "color": false})).unwrap();
        assert_eq!(px(&s)[0], Color32::from_rgb(0x33, 0x66, 0xff), "colour off: just the material");
    }

    #[test]
    fn varnish_is_glossy_and_stacked_varnish_has_relief() {
        let mut s = doc_with(&[("W3", [10, 2, 4, 4]), ("W4", [10, 2, 4, 4])]);
        s.execute("view.printPreview", json!({"substrate": "black", "white": false})).unwrap();
        let p = px(&s);
        let bare = p[16 * 7 + 15];
        let varnished = p[16 * 4 + 12];
        assert!(varnished.r() > bare.r() + 10, "gloss over the varnish: {varnished:?} vs {bare:?}");
        // Top-left edge catches the light, bottom-right edge is in shade.
        let lit = p[16 * 2 + 10];
        let shade = p[16 * 6 + 14];
        assert!(lit.r() > shade.r(), "relief: lit {lit:?}, shaded {shade:?}");
        s.execute("view.printPreview", json!({"varnish": false})).unwrap();
        let p = px(&s);
        assert_eq!(p[16 * 4 + 12], p[16 * 7 + 15], "varnish off: bare material");
    }

    #[test]
    fn partial_redraws_match_the_full_picture() {
        let mut s = doc_with(&[("W1", [0, 0, 16, 5]), ("W3", [3, 1, 9, 5]), ("W4", [5, 2, 4, 3])]);
        s.execute("view.printPreview", json!({"substrate": "clear"})).unwrap();
        let st = s.active().unwrap();
        let b = st.doc.bounds();
        for f in [1u32, 2] {
            let all = view(&st.doc, &st.channel_view, b, f, Look::default(), st.active_layer).unwrap();
            let w = b.width().div_ceil(f) as usize;
            let r = photocraft_geom::Rect::new(2 * f as i32, 2 * f as i32, 13, 7);
            let part = view(&st.doc, &st.channel_view, r, f, Look::default(), st.active_layer).unwrap();
            let pw = r.width().div_ceil(f) as usize;
            for (i, p) in part.iter().enumerate() {
                assert_eq!(*p, all[(i / pw + 2) * w + i % pw + 2], "factor {f}, pixel {i}");
            }
        }
    }
}
