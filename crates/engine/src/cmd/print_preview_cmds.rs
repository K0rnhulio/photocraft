//! View › print preview for UV flatbed printing: the canvas shows the image as it will look
//! printed on a material, with white ink under the colour and varnish (gloss) on top.
//!
//! UV printers driven by a RIP such as MainTop take the extra inks from spot channels named by
//! convention: `W1` and `W2` are white ink, `W3` and `W4` are varnish. The preview finds those
//! channels by name ([`ink_role`]) and stacks, bottom to top: the material, white ink, colour
//! ink, varnish. Colour inks are translucent, so they only look right over white ink or a white
//! material; on black or clear material without white underneath they fade, as they do in print.
//!
//! This is view state on [`crate::channel_cmds::ChannelView::print_preview`], like the Channels
//! panel's eyes: no history step, a clean document stays clean, and it never changes a pixel.

use serde::Serialize;
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

pub const ID: &str = "view.printPreview";

/// The material printed on.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Substrate {
    White,
    Black,
    /// Clear acrylic, glass or film: a checkerboard shows through where nothing is printed.
    Clear,
    /// Any other material colour (sRGB, 0..1).
    Color([f32; 3]),
}

impl Substrate {
    fn name(self) -> Value {
        match self {
            Self::White => json!("white"),
            Self::Black => json!("black"),
            Self::Clear => json!("clear"),
            Self::Color([r, g, b]) => json!(format!("#{:02x}{:02x}{:02x}", q(r), q(g), q(b))),
        }
    }
}

fn q(x: f32) -> u8 {
    (x.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// What the print preview shows.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrintPreview {
    pub substrate: Substrate,
    /// Show the white ink channels (W1, W2).
    pub white: bool,
    /// Show the colour ink (the image itself).
    pub color: bool,
    /// Show the varnish channels (W3, W4).
    pub varnish: bool,
}

impl Default for PrintPreview {
    fn default() -> Self {
        Self { substrate: Substrate::White, white: true, color: true, varnish: true }
    }
}

/// What an extra channel prints as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InkRole {
    White,
    Varnish,
}

/// The ink a channel prints by its name: `W1`/`W2` (or a name with "white") are white ink,
/// `W3`/`W4` (or "varnish", "gloss", "lack") are varnish. Case and surrounding spaces don't
/// matter. Other channels are not part of the print preview.
pub fn ink_role(name: &str) -> Option<InkRole> {
    let n = name.trim().to_lowercase();
    match n.as_str() {
        "w1" | "w2" => return Some(InkRole::White),
        "w3" | "w4" => return Some(InkRole::Varnish),
        _ => {}
    }
    if n.contains("white") || n.contains("weiss") || n.contains("weiß") {
        Some(InkRole::White)
    } else if ["varnish", "gloss", "lack"].iter().any(|w| n.contains(w)) {
        Some(InkRole::Varnish)
    } else {
        None
    }
}

fn bad(msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: ID.into(), msg: msg.into() }
}

fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document open".into())
}

fn flag(p: &Value, key: &str) -> Result<Option<bool>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_bool().map(Some).ok_or_else(|| bad(format!("`{key}` must be true or false"))),
    }
}

fn substrate_param(v: &Value) -> Result<Substrate> {
    let err = || bad("`substrate` must be \"white\", \"black\", \"clear\", a \"#rrggbb\" colour or [r, g, b] (0..1)");
    match v {
        Value::String(s) => match s.trim().to_lowercase().as_str() {
            "white" => Ok(Substrate::White),
            "black" => Ok(Substrate::Black),
            "clear" | "transparent" => Ok(Substrate::Clear),
            hex => crate::commands::parse_hex(hex).map(|c| Substrate::Color([c[0], c[1], c[2]])).ok_or_else(err),
        },
        Value::Array(a) if a.len() == 3 => {
            let mut c = [0.0f32; 3];
            for (slot, x) in c.iter_mut().zip(a) {
                let x = x.as_f64().filter(|x| x.is_finite()).ok_or_else(err)?;
                *slot = x.clamp(0.0, 1.0) as f32;
            }
            Ok(Substrate::Color(c))
        }
        _ => Err(err()),
    }
}

/// The preview as JSON (`null` when off); part of `channel.list` and `document.inspect`.
pub fn view_json(p: Option<PrintPreview>) -> Value {
    match p {
        Some(p) => json!({"substrate": p.substrate.name(), "white": p.white, "color": p.color, "varnish": p.varnish}),
        None => Value::Null,
    }
}

/// `{"on":bool?,"substrate":..?,"white":bool?,"color":bool?,"varnish":bool?}`. With no `on`, any
/// other setting turns the preview on; no params at all toggle it.
fn set_view(s: &mut Session, p: &Value) -> Result<Value> {
    let p = match p {
        Value::Object(_) => p.clone(),
        Value::Null => json!({}),
        _ => return Err(bad("expected an object")),
    };
    let on = flag(&p, "on")?;
    let substrate = match p.get("substrate") {
        None | Some(Value::Null) => None,
        Some(v) => Some(substrate_param(v)?),
    };
    let (white, color, varnish) = (flag(&p, "white")?, flag(&p, "color")?, flag(&p, "varnish")?);
    let settings = substrate.is_some() || white.is_some() || color.is_some() || varnish.is_some();
    let st = s.active_mut().ok_or(EngineError::NoDocument)?;
    let current = st.channel_view.print_preview;
    let mut next = current.unwrap_or_default();
    if let Some(sub) = substrate {
        next.substrate = sub;
    }
    next.white = white.unwrap_or(next.white);
    next.color = color.unwrap_or(next.color);
    next.varnish = varnish.unwrap_or(next.varnish);
    let show = match on {
        Some(on) => on,
        None if settings => true,
        None => current.is_none(),
    };
    let next = show.then_some(next);
    if next != current {
        st.channel_view.print_preview = next;
        // A view change: redraw without recompositing, and keep a clean document clean.
        let clean = st.saved_revision == st.revision;
        st.revision += 1;
        if clean {
            st.saved_revision = st.revision;
        }
        st.last_damage = Some(photocraft_geom::Rect::EMPTY);
    }
    let inks: Vec<Value> = st
        .doc
        .channels
        .iter()
        .filter_map(|c| {
            ink_role(&c.name).map(|r| json!({"name": c.name, "ink": if r == InkRole::White { "white" } else { "varnish" }, "spot": c.spot.is_some()}))
        })
        .collect();
    let mut out = view_json(next);
    if let Value::Object(m) = &mut out {
        m.insert("inks".into(), json!(inks));
    }
    Ok(if next.is_some() { out } else { json!({"inks": inks}) })
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: ID,
        label: "Print Preview",
        menu: &[],
        shortcut: None,
        params: r##"{"on":bool?,"substrate":"white"|"black"|"clear"|"#rrggbb"|[r,g,b]?,"white":bool?,"color":bool?,"varnish":bool?} (UV print preview: the image on the material, white ink from channels W1/W2 under the colour and varnish from W3/W4 shown as gloss on top; with no `on`, any setting turns it on and no params toggle it; view state, not an undo step; returns the settings, or null when off, and the white/varnish channels found)"##,
        enabled: has_doc,
        run: set_view,
        journal: false,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 8, "height": 8})).unwrap();
        s
    }

    #[test]
    fn channel_names_pick_the_ink() {
        assert_eq!(ink_role("W1"), Some(InkRole::White));
        assert_eq!(ink_role(" w2 "), Some(InkRole::White));
        assert_eq!(ink_role("W3"), Some(InkRole::Varnish));
        assert_eq!(ink_role("w4"), Some(InkRole::Varnish));
        assert_eq!(ink_role("Spot White"), Some(InkRole::White));
        assert_eq!(ink_role("Gloss Varnish"), Some(InkRole::Varnish));
        assert_eq!(ink_role("W5"), None);
        assert_eq!(ink_role("Alpha 1"), None);
    }

    #[test]
    fn toggles_and_keeps_a_clean_document_clean() {
        let mut s = session();
        let st = s.active().unwrap();
        assert!(st.channel_view.is_plain(&st.doc));
        let clean = st.saved_revision == st.revision;
        let out = s.execute(ID, json!({})).unwrap();
        assert_eq!(out["substrate"], "white");
        let st = s.active().unwrap();
        assert_eq!(st.channel_view.print_preview, Some(PrintPreview::default()));
        assert!(!st.channel_view.is_plain(&st.doc), "the preview draws over the canvas");
        assert_eq!(st.saved_revision == st.revision, clean);
        assert!(!st.history.can_undo(), "a view change is not an undo step");
        s.execute(ID, json!({})).unwrap();
        assert_eq!(s.active().unwrap().channel_view.print_preview, None);
    }

    #[test]
    fn settings_turn_it_on_and_stick() {
        let mut s = session();
        s.execute(ID, json!({"substrate": "black", "varnish": false})).unwrap();
        let p = s.active().unwrap().channel_view.print_preview.unwrap();
        assert_eq!((p.substrate, p.white, p.varnish), (Substrate::Black, true, false));
        s.execute(ID, json!({"substrate": "#ff0000"})).unwrap();
        let p = s.active().unwrap().channel_view.print_preview.unwrap();
        assert_eq!(p.substrate, Substrate::Color([1.0, 0.0, 0.0]));
        assert!(!p.varnish, "other settings are kept");
        s.execute(ID, json!({"substrate": [0.0, 0.5, 2.0]})).unwrap();
        assert_eq!(s.active().unwrap().channel_view.print_preview.unwrap().substrate, Substrate::Color([0.0, 0.5, 1.0]));
        s.execute(ID, json!({"on": false})).unwrap();
        assert_eq!(s.active().unwrap().channel_view.print_preview, None);
        let list = s.execute("channel.list", json!({})).unwrap();
        assert!(list["printPreview"].is_null());
    }

    #[test]
    fn reports_the_ink_channels_it_found() {
        let mut s = session();
        s.execute("channel.new", json!({"name": "W1"})).unwrap();
        s.execute("channel.new", json!({"name": "Alpha 2"})).unwrap();
        s.execute("channel.new", json!({"name": "W3"})).unwrap();
        let out = s.execute(ID, json!({"on": true})).unwrap();
        let inks = out["inks"].as_array().unwrap();
        assert_eq!(inks.len(), 2);
        assert_eq!((inks[0]["name"].as_str(), inks[0]["ink"].as_str()), (Some("W1"), Some("white")));
        assert_eq!((inks[1]["name"].as_str(), inks[1]["ink"].as_str()), (Some("W3"), Some("varnish")));
        let list = s.execute("channel.list", json!({})).unwrap();
        assert_eq!(list["printPreview"]["substrate"], "white");
    }

    #[test]
    fn bad_params_are_errors_not_panics() {
        let mut s = session();
        for p in [
            json!({"substrate": "plaid"}),
            json!({"substrate": 3}),
            json!({"substrate": [1, "x", 0]}),
            json!({"substrate": [1, 2]}),
            json!({"white": "yes"}),
            json!({"on": 1}),
            json!([1, 2]),
        ] {
            assert!(s.execute(ID, p.clone()).is_err(), "{p}");
        }
        assert_eq!(s.active().unwrap().channel_view.print_preview, None, "a failed call changes nothing");
        assert!(Session::new().execute(ID, json!({})).is_err(), "no document");
    }
}
