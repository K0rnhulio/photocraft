//! TIFF Options: the prompt File › Save As shows before writing a layered TIFF, as Photoshop
//! does when "Ask Before Saving Layered TIFF Files" is on in Preferences › File Handling, and
//! before writing a TIFF of a document with alpha or spot channels.
//!
//! The save is parked in [`Prompt`] while the modal is up (egui is immediate-mode) and written
//! once answered: with the layers (Photoshop layer data in the file), or flattened ("Discard
//! Layers and Save a Copy"); with or without the alpha channels and spot colours (Photoshop's
//! Save As checkboxes, which a RIP such as a UV printer's reads as ink plates); and with Save
//! Transparency, the transparency kept next to those channels instead of composited over white.
//! Leaving channels out saves a copy. "Don't show again" clears the layers preference. Cancel
//! drops the save.

use crate::{ExportSettings, PhotocraftApp};

/// A save waiting on the TIFF Options answer.
pub struct Prompt {
    /// The document that requested the save, even if tabs change while this is open.
    doc: photocraft_doc::DocId,
    /// Where the document goes.
    pub path: String,
    /// "Discard Layers and Save a Copy".
    pub discard_layers: bool,
    /// Clears `ask_before_saving_layered_tiff` when the save goes ahead.
    pub dont_ask_again: bool,
    /// Whether the layers question is asked (a layered TIFF with the preference on); else the
    /// prompt is up for the channels alone and the layers are kept.
    pub asks_layers: bool,
    /// The channel choices: Alpha Channels, Spot Colors, Save Transparency.
    pub channels: photocraft_io::TiffChannels,
    /// How many alpha channels (a Quick Mask counts) and spot channels the document has.
    pub counts: (usize, usize),
}

/// How many alpha channels (a Quick Mask counts) and spot channels `doc` has.
pub fn channel_counts(doc: &photocraft_doc::Document) -> (usize, usize) {
    let spots = doc.channels.iter().filter(|c| c.spot.is_some()).count();
    (doc.channels.len() - spots + usize::from(doc.quick_mask.is_some()), spots)
}

/// Whether saving `doc` with `choice` leaves any of its channels out (then the save is a copy).
pub fn drops_channels(doc: &photocraft_doc::Document, choice: photocraft_io::TiffChannels) -> bool {
    let (alphas, spots) = channel_counts(doc);
    (alphas > 0 && !choice.alpha_channels) || (spots > 0 && !choice.spot_colors)
}

/// The channel choices in a save command's params (see
/// [`photocraft_engine::file_cmds::tiff_channels_param`]).
pub fn channels_param(params: &serde_json::Value, cmd: &str) -> Result<Option<photocraft_io::TiffChannels>, String> {
    photocraft_engine::file_cmds::tiff_channels_param(params, cmd).map_err(|e| e.to_string())
}

/// Adds the channel choices to a recorded save step.
pub fn record(t: photocraft_io::TiffChannels, step: &mut serde_json::Value) {
    step["tiffAlphaChannels"] = t.alpha_channels.into();
    step["tiffSpotColors"] = t.spot_colors.into();
    step["tiffSaveTransparency"] = t.save_transparency.into();
}

/// `true` when `path` is a TIFF name (`.tif` / `.tiff`).
pub fn is_tiff_path(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    name.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && (ext.eq_ignore_ascii_case("tif") || ext.eq_ignore_ascii_case("tiff")))
}

/// Whether the layers question applies to saving the active document to `path`: a TIFF that
/// would carry layer data, with the preference on.
fn asks_layers(app: &PhotocraftApp, path: &str) -> bool {
    is_tiff_path(path)
        && app.session.prefs().file_handling.ask_before_saving_layered_tiff
        && app.session.active().is_some_and(|st| photocraft_engine::file_cmds::tiff_would_write_layers(&st.doc))
}

/// Whether saving the active document to `path` should go through the prompt: a TIFF that would
/// carry layer data, with the preference on, or a TIFF of a document with alpha or spot channels
/// whose channel choices (`channels_answered`) the caller hasn't given.
pub fn wants_prompt(app: &PhotocraftApp, path: &str, channels_answered: bool) -> bool {
    asks_layers(app, path) || (is_tiff_path(path) && !channels_answered && app.session.active().is_some_and(|st| channel_counts(&st.doc) != (0, 0)))
}

/// Parks the save behind the prompt.
pub fn park(app: &mut PhotocraftApp, path: String) -> Result<(), String> {
    let doc = app.active_doc_id()?;
    let asks_layers = asks_layers(app, &path);
    let counts = app.session.active().map(|st| channel_counts(&st.doc)).unwrap_or_default();
    app.tiff_options =
        Some(Prompt { doc, path, discard_layers: false, dont_ask_again: false, asks_layers, channels: photocraft_io::TiffChannels::default(), counts });
    Ok(())
}

pub fn show(app: &mut PhotocraftApp, ctx: &egui::Context) {
    let Some(p) = &mut app.tiff_options else { return };
    let (mut ok, mut cancel) = (false, false);
    let (mut discard, mut dont_ask, mut ch) = (p.discard_layers, p.dont_ask_again, p.channels);
    let (asks_layers, (alphas, spots)) = (p.asks_layers, p.counts);
    let modal = egui::Modal::new(egui::Id::unique("tiff-options")).show(ctx, |ui| {
        ui.set_max_width(420.0);
        ui.label(egui::RichText::new(tl!("TIFF Options")).font(crate::theme::semibold(15.0)));
        ui.add_space(4.0);
        crate::widgets::hairline(ui);
        ui.add_space(8.0);
        if asks_layers {
            ui.label(tl!("Including layers will increase file size."));
            ui.add_space(8.0);
            crate::widgets::checkbox(ui, &mut discard, tl!("Discard Layers and Save a Copy"));
        }
        if alphas + spots > 0 {
            if asks_layers {
                ui.add_space(8.0);
            }
            // Each channel goes into the file as an extra channel a RIP reads by its name.
            if spots > 0 {
                let label = crate::i18n::fmt(tl!("Spot Colors ({count})"), &[("count", &spots.to_string())]);
                crate::widgets::checkbox(ui, &mut ch.spot_colors, &label);
            }
            if alphas > 0 {
                let label = crate::i18n::fmt(tl!("Alpha Channels ({count})"), &[("count", &alphas.to_string())]);
                crate::widgets::checkbox(ui, &mut ch.alpha_channels, &label);
            }
            crate::widgets::checkbox(ui, &mut ch.save_transparency, tl!("Save Transparency"));
            let kept = (ch.spot_colors && spots > 0) || (ch.alpha_channels && alphas > 0);
            let note = if (spots > 0 && !ch.spot_colors) || (alphas > 0 && !ch.alpha_channels) {
                tl!("Channels left out: the file is saved as a copy.")
            } else if kept && !ch.save_transparency {
                tl!("Transparent areas are saved on white next to the channels.")
            } else {
                ""
            };
            if !note.is_empty() {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(note).small());
            }
        }
        if asks_layers {
            crate::widgets::checkbox(ui, &mut dont_ask, tl!("Don't show again"));
        }
        ui.add_space(12.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            ok = crate::widgets::primary_button(ui, tl!("OK"), 84.0).clicked();
            cancel = crate::widgets::secondary_button(ui, tl!("Cancel"), 84.0).clicked();
        });
    });
    cancel |= modal.should_close();
    p.discard_layers = discard;
    p.dont_ask_again = dont_ask;
    p.channels = ch;
    // A focused button takes Enter as its click without consuming the key: Enter on Cancel cancels.
    if ok || (!cancel && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))) {
        let Some(p) = app.tiff_options.take() else { return };
        if p.dont_ask_again {
            app.session.edit_prefs(|prefs| prefs.file_handling.ask_before_saving_layered_tiff = false);
        }
        let settings = ExportSettings { tiff_layers: !p.discard_layers, tiff_channels: p.channels, ..app.save_settings() };
        let mut step = serde_json::json!({"path": p.path, "tiffLayers": !p.discard_layers});
        let asked_channels = p.counts != (0, 0);
        if asked_channels {
            record(p.channels, &mut step);
        }
        let dropped = asked_channels && (p.counts.1 > 0 && !p.channels.spot_colors || p.counts.0 > 0 && !p.channels.alpha_channels);
        let copy = p.discard_layers || dropped;
        let written = app.with_document(p.doc, |app| app.write_document(p.path, &settings, copy));
        if written.is_ok() {
            // An action being recorded keeps the Save As with this answer (#2032).
            app.session.journal.push(("file.saveAs".into(), step));
        }
        match written {
            Err(e) => {
                app.ui.status = crate::i18n::fmt(tl!("Save failed: {error}"), &[("error", &e)]);
                app.ui.status_error = true;
            }
            Ok(Some(_)) if !copy => crate::discard_ui::saved_document(app, ctx, p.doc),
            // A background save releases the close prompt when it finishes (`jobs_ui`).
            Ok(_) => {}
        }
    } else if cancel {
        app.tiff_options = None;
        app.ui.status = tl!("Save cancelled").into();
        app.ui.status_error = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiff_names() {
        assert!(is_tiff_path("a.tif"));
        assert!(is_tiff_path("dir/sub/a.TIFF"));
        assert!(is_tiff_path("C:\\x\\y.tiff"));
        assert!(!is_tiff_path("a.tif.psd"));
        assert!(!is_tiff_path(".tif"));
        assert!(!is_tiff_path("tif"));
        assert!(!is_tiff_path("a.png"));
    }
}
