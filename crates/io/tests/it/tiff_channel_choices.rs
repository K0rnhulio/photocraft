//! TIFF Options' channel choices: Save As › Alpha Channels and Spot Colors leave those channels
//! out. Save Transparency is upstream's `tiff_transparency` (see `tiff_save_options`).

use photocraft_color::{Color, ColorMode, PixelFormat, SampleType};
use photocraft_doc::{AlphaChannel, Document, Layer, LayerContent};
use photocraft_geom::{Rect, Size};
use photocraft_io::*;
use photocraft_raster::Surface;

/// A 4×4 RGB document, transparent outside its middle, with a spot channel ("Gold", ink in the
/// left half), an alpha channel ("Mask", white in the top half) and a Quick Mask.
fn doc() -> Document {
    let mut doc = Document::new("c", Size::new(4, 4), ColorMode::Rgb, SampleType::U8);
    let fmt = doc.pixel_format();
    let mut art = Surface::new(fmt);
    art.fill_rect(Rect::new(1, 1, 3, 3), &photocraft_raster::from_rgba(&fmt, [1.0, 0.0, 0.0, 1.0]));
    doc.layers.push(Layer::new("Layer 1", LayerContent::Raster(art)));
    let gray = PixelFormat::new(ColorMode::Grayscale, SampleType::U8, false);
    let white = photocraft_raster::from_rgba(&gray, [1.0, 1.0, 1.0, 1.0]);
    let mut ink = Surface::new(gray);
    ink.fill_rect(Rect::new(0, 0, 2, 4), &white);
    let mut gold = AlphaChannel::new("Gold", ink);
    gold.spot = Some((Color::rgb(0.83, 0.69, 0.22), 0.25));
    doc.channels.push(gold);
    let mut mask = Surface::new(gray);
    mask.fill_rect(Rect::new(0, 0, 4, 2), &white);
    doc.channels.push(AlphaChannel::new("Mask", mask));
    doc.quick_mask = Some(AlphaChannel::new("Quick Mask", Surface::new(gray)));
    doc
}

fn save(doc: &Document, layers: bool, tiff_channels: TiffChannels) -> ExportResult {
    export(doc, "c.tif", &ExportOptions { tiff_layers: layers, tiff_channels, ..Default::default() }).expect("export")
}

fn extra_samples_tag(bytes: &[u8]) -> Vec<u16> {
    let mut dec = tiff::decoder::Decoder::new(std::io::Cursor::new(bytes)).expect("tiff");
    dec.get_tag_u16_vec(tiff::tags::Tag::ExtraSamples).unwrap_or_default()
}

fn names(d: &Document) -> Vec<&str> {
    d.channels.iter().map(|c| c.name.as_str()).collect()
}

#[test]
fn defaults_keep_every_channel_and_no_transparency() {
    assert_eq!(TiffChannels::default(), TiffChannels { alpha_channels: true, spot_colors: true });
    for layers in [false, true] {
        let r = save(&doc(), layers, TiffChannels::default());
        assert_eq!(extra_samples_tag(&r.bytes), [0, 0, 0], "{layers}: spot, alpha, Quick Mask");
        let back = import("c.tif", &r.bytes).unwrap().document;
        assert_eq!(names(&back), ["Gold", "Mask"], "{layers}");
        assert!(back.quick_mask.is_some(), "{layers}");
    }
}

#[test]
fn spot_colors_off_leaves_the_spot_channels_out() {
    let choice = TiffChannels { spot_colors: false, ..Default::default() };
    for layers in [false, true] {
        let r = save(&doc(), layers, choice);
        assert!(r.warnings.iter().any(|w| w.contains("Spot Colors off") && w.contains("\"Gold\"")), "{layers}: {:?}", r.warnings);
        assert_eq!(extra_samples_tag(&r.bytes), [0, 0], "{layers}: alpha, Quick Mask");
        let back = import("c.tif", &r.bytes).unwrap().document;
        assert_eq!(names(&back), ["Mask"], "{layers}");
        assert!(back.channels[0].spot.is_none(), "{layers}");
        assert!(back.quick_mask.is_some(), "{layers}");
    }
}

#[test]
fn alpha_channels_off_leaves_alpha_channels_and_the_quick_mask_out() {
    let choice = TiffChannels { alpha_channels: false, ..Default::default() };
    for layers in [false, true] {
        let r = save(&doc(), layers, choice);
        assert!(r.warnings.iter().any(|w| w.contains("Alpha Channels off") && w.contains("\"Mask\"")), "{layers}: {:?}", r.warnings);
        assert!(r.warnings.iter().any(|w| w.contains("Quick Mask not saved")), "{layers}: {:?}", r.warnings);
        assert_eq!(extra_samples_tag(&r.bytes), [0], "{layers}: the spot alone");
        let back = import("c.tif", &r.bytes).unwrap().document;
        assert_eq!(names(&back), ["Gold"], "{layers}");
        let (ink, solidity) = back.channels[0].spot.expect("spot");
        assert!((solidity - 0.25).abs() < 1e-4 && (ink.to_rgb()[0] - 0.83).abs() < 0.01, "{layers}");
        assert!(back.quick_mask.is_none(), "{layers}");
    }
}

#[test]
fn both_off_writes_the_image_alone_with_its_transparency() {
    let choice = TiffChannels { alpha_channels: false, spot_colors: false };
    let r = save(&doc(), false, choice);
    // No extra channel left: a plain transparent TIFF, nothing matted.
    assert_eq!(extra_samples_tag(&r.bytes), [2]);
    assert!(!r.warnings.iter().any(|w| w.contains("composited over white")), "{:?}", r.warnings);
    let back = import("c.tif", &r.bytes).unwrap().document;
    assert!(back.channels.is_empty() && back.quick_mask.is_none());
}
