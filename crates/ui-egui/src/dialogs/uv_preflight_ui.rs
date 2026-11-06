//! File › UV Print Preflight…: shows the `file.uvPreflight` report in a dialog.
//!
//! The engine does the checking (see `photocraft_engine::uv_preflight_cmds`); this dialog lays
//! out its report: the print size, a table of extra channels with their role and ink coverage,
//! then the errors, warnings and notes. It changes nothing, so it only has OK (and ↩ / Esc). It
//! is an ordinary dialog in the UI state, so `ui.inspect` shows the report and
//! `ui.dialog.confirm` closes it. Check Again re-runs the check after the user fixed something.

use egui::RichText;
use serde_json::{Map, Value, json};

use crate::PhotocraftApp;
use crate::state::DialogKind;
use crate::theme::Tokens;

const MARK: &str = "__uvPreflight";
pub const COMMAND: &str = "file.uvPreflight";
/// Set by Check Again; the next frame runs the check again into the same dialog.
const RERUN: &str = "__rerun";

pub fn owns(fields: &Map<String, Value>) -> bool {
    fields.contains_key(MARK)
}

/// Runs the check on the active document and opens the dialog with its report.
pub fn open(app: &mut PhotocraftApp) -> Result<Value, String> {
    let report = app.run(COMMAND, json!({}))?;
    let mut f = Map::new();
    f.insert(MARK.into(), json!(true));
    f.insert("__label".into(), json!("UV Print Preflight"));
    f.insert("report".into(), report.clone());
    let dialog = app.ui.open_dialog(DialogKind::Command, f);
    Ok(json!({"dialog": dialog, "report": report}))
}

/// Check Again: replaces the dialog's report with a fresh one.
pub fn rerun_requested(app: &mut PhotocraftApp, id: u64) {
    let wants = app.ui.dialog_mut(id).is_some_and(|d| d.fields.remove(RERUN).is_some());
    if !wants {
        return;
    }
    let report = app.run(COMMAND, json!({}));
    match report {
        Ok(report) => {
            if let Some(d) = app.ui.dialog_mut(id) {
                d.fields.insert("report".into(), report);
            }
        }
        Err(e) => app.ui.status = e,
    }
}

fn num(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).filter(|f| f.is_finite()).unwrap_or(0.0)
}

/// "297 × 210 mm  ·  11.69 × 8.27 in" for the size block, or "unknown" without a resolution.
fn physical(size: &Value) -> String {
    if num(size, "dpi") <= 0.0 {
        return tl!("unknown (no resolution)").to_string();
    }
    format!("{:.1} × {:.1} mm  ·  {:.2} × {:.2} in", num(size, "widthMm"), num(size, "heightMm"), num(size, "widthIn"), num(size, "heightIn"))
}

fn role_label(role: Option<&str>) -> &'static str {
    match role {
        Some("white") => tl!("White"),
        Some("varnish") => tl!("Varnish"),
        _ => "—",
    }
}

pub fn body(ui: &mut egui::Ui, fields: &mut Map<String, Value>) {
    let t = Tokens::get(ui.ctx());
    let report = fields.get("report").cloned().unwrap_or(Value::Null);
    ui.set_min_width(460.0);

    // Verdict first: what the user wants to know at a glance.
    let (verdict, color) = match report["status"].as_str() {
        Some("ready") => (tl!("Ready to print: no problems found."), t.text),
        Some("fix") => (tl!("Fix the errors below before printing."), t.danger),
        _ => (tl!("Check the warnings below before printing."), t.warning),
    };
    ui.label(RichText::new(verdict).strong().color(color));
    ui.add_space(8.0);

    let size = &report["size"];
    egui::Grid::new("uv-preflight-size").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
        ui.label(RichText::new(tl!("Pixels")).color(t.text_dim));
        ui.label(format!("{} × {} px", num(size, "width"), num(size, "height")));
        ui.end_row();
        ui.label(RichText::new(tl!("Resolution")).color(t.text_dim));
        let dpi = num(size, "dpi");
        let low = dpi > 0.0 && dpi < num(size, "minDpi");
        ui.label(RichText::new(format!("{dpi:.0} ppi")).color(if low { t.warning } else { t.text }));
        ui.end_row();
        ui.label(RichText::new(tl!("Print size")).color(t.text_dim));
        ui.label(physical(size));
        ui.end_row();
        ui.label(RichText::new(tl!("Mode")).color(t.text_dim));
        ui.label(format!("{} · {}-bit", report["mode"].as_str().unwrap_or("?"), num(&report, "depth")));
        ui.end_row();
    });

    ui.add_space(10.0);
    ui.label(RichText::new(tl!("Channels")).strong());
    let channels = report["channels"].as_array().cloned().unwrap_or_default();
    if channels.is_empty() {
        ui.label(RichText::new(tl!("No spot or alpha channels.")).color(t.text_dim));
    } else {
        egui::Grid::new("uv-preflight-channels").num_columns(4).striped(true).spacing([16.0, 4.0]).show(ui, |ui| {
            for h in [tl!("Name"), tl!("Type"), tl!("Prints as"), tl!("Ink coverage")] {
                ui.label(RichText::new(h).color(t.text_dim));
            }
            ui.end_row();
            for c in &channels {
                let role = c["role"].as_str();
                ui.label(RichText::new(c["name"].as_str().unwrap_or("")).color(if role.is_some() { t.text } else { t.warning }));
                ui.label(if c["kind"] == "spot" { tl!("Spot") } else { tl!("Alpha") });
                ui.label(role_label(role));
                let empty = c["empty"] == true;
                ui.label(RichText::new(c["inkedText"].as_str().unwrap_or("")).color(if empty && role.is_some() { t.warning } else { t.text }));
                ui.end_row();
            }
        });
    }

    let checks = report["checks"].as_array().cloned().unwrap_or_default();
    if !checks.is_empty() {
        ui.add_space(10.0);
        ui.label(RichText::new(tl!("Checks")).strong());
        for c in &checks {
            let (icon, color) = match c["level"].as_str() {
                Some("error") => ("ban", t.danger),
                Some("warning") => ("triangle-alert", t.warning),
                _ => ("info", t.text_dim),
            };
            ui.horizontal_wrapped(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                crate::icons::paint(ui, r, icon, 12.0, color);
                ui.label(c["message"].as_str().unwrap_or(""));
            });
        }
    }

    ui.add_space(8.0);
    if ui.button(tl!("Check Again")).clicked() {
        fields.insert(RERUN.into(), json!(true));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_with_the_report_and_closes_without_changes() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.run("file.new", json!({"width": 30, "height": 20, "resolution": 300})).unwrap();
        app.run("channel.newSpot", json!({"name": "W1", "fromSelection": false})).unwrap();
        let steps = app.session.active().unwrap().history.past_len();
        let r = crate::menus::invoke(&mut app, &egui::Context::default(), COMMAND, json!({})).unwrap();
        let id = r["dialog"].as_u64().unwrap();
        assert_eq!(r["report"]["channels"][0]["role"], "white");
        let d = app.ui.dialogs.iter().find(|d| d.id == id).unwrap();
        assert!(owns(&d.fields));
        assert_eq!(d.fields["report"]["size"]["width"], 30);
        // W1 is empty: a warning, so the verdict asks to check.
        assert_eq!(d.fields["report"]["status"], "check");
        crate::dialogs::confirm(&mut app, id).unwrap();
        assert!(app.ui.dialogs.is_empty());
        assert_eq!(app.session.active().unwrap().history.past_len(), steps, "the check is not an undo step");
    }

    #[test]
    fn check_again_refreshes_the_report() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.run("file.new", json!({"width": 10, "height": 10, "resolution": 300})).unwrap();
        let r = open(&mut app).unwrap();
        let id = r["dialog"].as_u64().unwrap();
        app.run("channel.newSpot", json!({"name": "W1", "fromSelection": false})).unwrap();
        app.ui.dialog_mut(id).unwrap().fields.insert(RERUN.into(), json!(true));
        rerun_requested(&mut app, id);
        let d = app.ui.dialogs.iter().find(|d| d.id == id).unwrap();
        assert_eq!(d.fields["report"]["channels"][0]["name"], "W1");
        assert!(!d.fields.contains_key(RERUN));
    }

    #[test]
    fn without_a_document_it_is_an_error() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        assert!(crate::menus::invoke(&mut app, &egui::Context::default(), COMMAND, json!({})).is_err());
        assert!(app.ui.dialogs.is_empty());
    }

    #[test]
    fn physical_size_needs_a_resolution() {
        assert!(physical(&json!({"dpi": 0})).contains("unknown"));
        assert_eq!(
            physical(&json!({"dpi": 300, "widthMm": 254.0, "heightMm": 127.0, "widthIn": 10.0, "heightIn": 5.0})),
            "254.0 × 127.0 mm  ·  10.00 × 5.00 in"
        );
    }
}
