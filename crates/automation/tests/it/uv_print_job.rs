//! A flatbed UV print job driven over MCP the way an agent would: open the artwork, fit it to
//! 100 x 160 mm on a 120 x 180 mm page, make the white underbase (W1) under everything that isn't
//! paper and gloss varnish (W3) over the black and red, run UV Print Preflight and save a TIFF
//! whose spot channels survive a reopen.

use std::io::Write;

use photocraft_automation::{AuthorizedWorkspace, PhotocraftMcp};
use rmcp::model::{CallToolRequestParams, CallToolResult, ClientConfig};
use rmcp::service::RunningService;
use rmcp::{ClientHandler, RoleClient, ServiceExt};
use serde_json::{Value, json};

#[derive(Clone, Default)]
struct Client;
impl ClientHandler for Client {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::default()
    }
}

type Conn = RunningService<RoleClient, Client>;
type Server = tokio::task::JoinHandle<()>;

async fn connect(root: &std::path::Path) -> (Conn, Server) {
    let (s, c) = tokio::io::duplex(1 << 20);
    let workspace = AuthorizedWorkspace::new(Some(root), Some(root)).expect("test workspace");
    let server = tokio::spawn(async move {
        if let Ok(running) = PhotocraftMcp::headless_with_workspace(workspace).serve(s).await {
            let _ = running.waiting().await;
        }
    });
    (Client.serve(c).await.expect("client init"), server)
}

/// Waits for the server task, so its open directory handles are gone before the folder is
/// removed (Windows refuses otherwise).
async fn shutdown(c: Conn, server: Server) {
    c.cancel().await.unwrap();
    server.await.unwrap();
}

fn text(r: &CallToolResult) -> String {
    r.content.iter().filter_map(|c| c.as_text()).map(|t| t.text.clone()).collect::<Vec<_>>().join("\n")
}

async fn tool(c: &Conn, name: &str, args: Value) -> Value {
    let mut p = CallToolRequestParams::new(name.to_owned());
    if let Value::Object(m) = args {
        p = p.with_arguments(m);
    }
    let r = c.call_tool(p).await.expect("call_tool transport");
    assert_ne!(r.is_error, Some(true), "{name} failed: {}", text(&r));
    serde_json::from_str(&text(&r)).unwrap_or(Value::String(text(&r)))
}

async fn run(c: &Conn, id: &str, params: Value) -> Value {
    tool(c, "command_run", json!({"id": id, "params": params})).await
}

/// A 300 x 480 px artwork on white with a wide white margin: a black block and a red block
/// (to be glossed) and a light blue block (white underbase only).
fn artwork_png(dir: &std::path::Path) -> &'static str {
    let (w, h) = (300u32, 480u32);
    let mut px = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let c: [u8; 3] = match (x, y) {
                (60..=239, 60..=179) => [10, 10, 10],
                (60..=239, 180..=299) => [200, 20, 30],
                (60..=239, 300..=419) => [150, 200, 240],
                _ => [255, 255, 255],
            };
            px.extend_from_slice(&c);
        }
    }
    let img = photocraft_codecs::Image::from_u8(w, h, photocraft_codecs::ChannelLayout::Rgb, px).unwrap();
    std::fs::File::create(dir.join("artwork.png"))
        .unwrap()
        .write_all(&photocraft_codecs::encode(&img, photocraft_codecs::Format::Png, &Default::default()).unwrap())
        .unwrap();
    "artwork.png"
}

fn channel<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["channels"].as_array().and_then(|a| a.iter().find(|c| c["name"] == name)).unwrap_or_else(|| panic!("no {name} channel: {report}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn uv_print_job_from_artwork_to_tiff() {
    let dir = std::env::temp_dir().join(format!("pc-uv-job-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let png = artwork_png(&dir);
    let (c, server) = connect(&dir).await;

    tool(&c, "doc_open", json!({"path": png})).await;

    // Trim the white margin, fit 100 x 160 mm, centre on 120 x 180 mm at 300 ppi.
    let fit = run(&c, "image.fitToPrint", json!({"width": 120, "height": 180, "artWidth": 100, "artHeight": 160, "trim": "white"})).await;
    assert_eq!(fit["width"], 1417, "{fit}");
    assert_eq!(fit["height"], 2126, "{fit}");
    assert_eq!(fit["trimmed"], json!({"x": 60, "y": 60, "width": 180, "height": 360}), "{fit}");
    assert_eq!(fit["artwork"]["heightMm"], 160.0, "{fit}");

    // White underbase: everything that isn't paper white.
    run(&c, "select.colorRange", json!({"color": "#ffffff", "fuzziness": 20, "invert": true})).await;
    run(&c, "channel.uvInk", json!({"name": "W1", "source": "selection"})).await;
    // Gloss varnish over the black and the red only.
    run(&c, "select.colorRange", json!({"colors": ["#0a0a0a", "#c8141e"], "fuzziness": 30})).await;
    run(&c, "channel.uvInk", json!({"name": "W3", "source": "selection"})).await;
    run(&c, "select.deselect", json!({})).await;

    let report = run(&c, "file.uvPreflight", json!({})).await;
    assert_ne!(report["status"], "fix", "{report}");
    assert_eq!(report["size"]["widthMm"].as_f64().map(f64::round), Some(120.0), "{report}");
    let (w1, w3) = (channel(&report, "W1"), channel(&report, "W3"));
    assert_eq!(w1["kind"], "spot");
    assert_eq!(w1["role"], "white");
    assert_eq!(w3["role"], "varnish");
    // The varnish covers two of the artwork's three blocks.
    let (m1, m3) = (w1["mean"].as_f64().unwrap(), w3["mean"].as_f64().unwrap());
    assert!(m1 > 0.0 && m3 > 0.0 && m3 < m1, "W1 {m1}, W3 {m3}");
    assert!((m3 / m1 - 2.0 / 3.0).abs() < 0.05, "varnish should be about 2/3 of the underbase: W1 {m1}, W3 {m3}");

    // Save the print file and check the spot channels come back from it.
    tool(&c, "doc_save", json!({"path": "print.tif"})).await;
    tool(&c, "doc_open", json!({"path": "print.tif"})).await;
    let reopened = run(&c, "file.uvPreflight", json!({})).await;
    assert_eq!(channel(&reopened, "W1")["kind"], "spot", "{reopened}");
    assert_eq!(channel(&reopened, "W3")["kind"], "spot", "{reopened}");
    assert_eq!(reopened["size"]["width"], 1417, "{reopened}");

    shutdown(c, server).await;
    let _ = std::fs::remove_dir_all(&dir);
}
