use std::io::Write;
use std::net::{Ipv4Addr, SocketAddr};

use serde_json::Value;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use photocraft_automation::{Backend, BridgeClient, Headless, Launcher, PhotocraftMcp, security};

use crate::{
    Result,
    args::{Bridge, CommandsArgs, McpArgs, ServeArgs},
    output::{JsonStyle, print_json},
};

pub(crate) fn commands(a: &CommandsArgs, out: &mut dyn Write) -> Result {
    let h = Headless::new();
    let list = h.command_list();
    let needle = a.filter.as_deref().map(str::to_lowercase);
    let items: Vec<&Value> = list
        .as_array()
        .map(|v| v.iter().collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter(|c| needle.as_ref().is_none_or(|n| photocraft_engine::commands::list_entry_matches(c, n)))
        .collect();
    if a.json {
        return print_json(out, &Value::Array(items.into_iter().cloned().collect()), JsonStyle::Pretty);
    }
    for c in items {
        let menu = c["menu"].as_array().map(|m| m.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" > ")).unwrap_or_default();
        writeln!(out, "{:<44} {:<32} {}", c["id"].as_str().unwrap_or(""), c["label"].as_str().unwrap_or(""), menu)?;
    }
    Ok(())
}

pub(crate) fn serve(a: &ServeArgs, err: &mut dyn Write) -> Result {
    use std::sync::{Arc, Mutex};
    let h = Arc::new(Mutex::new(Headless::with_workspace(a.automation.workspace()?)));
    match a.port {
        Some(port) => {
            if a.port_from_env {
                // Name the mode switch: an exported variable changes what this command does, and
                // unsetting it (or setting it empty) is the way back to stdio.
                let _ = writeln!(err, "photocraft-cli: PHOTOCRAFT_CONTROL_PORT={port} is set; serving on TCP instead of stdio (unset it to serve stdio)");
            }
            let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port)).to_string();
            let (supplied, token_file) = (a.control.control_token.as_deref(), a.control.control_token_file.as_deref());
            let token = security::server_token(supplied, token_file)?;
            if let Some(path) = token_file {
                let _ = writeln!(err, "photocraft-cli control token file: {}", path.display());
            } else if supplied.is_none() {
                let _ = writeln!(err, "photocraft-cli control token: {token}");
            }
            photocraft_automation::rpc::serve_tcp(&addr, h, token, |local| {
                let _ = writeln!(err, "photocraft-cli serving on {local}");
            })?;
            Ok(())
        }
        None => {
            let stdin = std::io::stdin();
            Ok(photocraft_automation::rpc::serve_lines(&h, stdin.lock(), std::io::stdout())?)
        }
    }
}

pub(crate) fn mcp(a: &McpArgs, err: &mut dyn Write) -> Result {
    let server = match &a.bridge {
        Some(addr) => {
            if a.bridge_from_env {
                // Name the mode switch, as `serve` does: the way back to headless is to unset it.
                let _ = writeln!(
                    err,
                    "photocraft-cli: PHOTOCRAFT_CONTROL_PORT is set; bridging to the app on {addr} instead of a headless engine (unset it to run headless)"
                );
            }
            match (&a.launch, a.control.control_token_file.as_deref()) {
                (Some(app), Some(token_file)) => {
                    // The app this may start reads the token from the file, so create it now
                    // when it is missing, as the app itself would.
                    let token = security::server_token(None, Some(token_file))?;
                    let launcher = launcher(Path::new(app), addr, token_file, a)?;
                    PhotocraftMcp::with_backend(Backend::Bridge(Arc::new(BridgeClient::new(addr.to_string(), token)?.with_launcher(launcher))))
                }
                _ => {
                    let token = security::client_token(a.control.control_token.as_deref(), a.control.control_token_file.as_deref())?;
                    PhotocraftMcp::bridge(&addr.to_string(), &token)?
                }
            }
        }
        None => PhotocraftMcp::headless_with_workspace(a.automation.workspace()?),
    };
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    Ok(rt.block_on(server.serve_stdio())?)
}

/// `mcp --launch`: the app to start (`app`, or the `photocraft` beside this program when empty)
/// and its options: control on the bridge's port, the token file and the automation roots.
fn launcher(app: &Path, bridge: &Bridge, token_file: &Path, a: &McpArgs) -> Result<Launcher> {
    let program = if app.as_os_str().is_empty() {
        let exe = std::env::current_exe()?;
        exe.with_file_name(format!("photocraft{}", std::env::consts::EXE_SUFFIX))
    } else {
        app.to_path_buf()
    };
    let port = match bridge {
        Bridge::Port(port) => *port,
        Bridge::Addr(addr) => addr.rsplit_once(':').and_then(|(_, p)| p.parse().ok()).unwrap_or(0),
    };
    if port == 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("--launch needs a port to start the app on, not `{bridge}`")).into());
    }
    let absolute = |p: &Path| -> std::io::Result<PathBuf> { std::path::absolute(p) };
    let mut args: Vec<OsString> = vec!["--control".into(), port.to_string().into(), "--control-token-file".into(), absolute(token_file)?.into()];
    if let Some(root) = &a.automation.automation_read_root {
        args.extend(["--automation-read-root".into(), absolute(root)?.into()]);
    }
    if let Some(root) = &a.automation.automation_write_root {
        args.extend(["--automation-write-root".into(), absolute(root)?.into()]);
    }
    Ok(Launcher::new(program, args))
}
