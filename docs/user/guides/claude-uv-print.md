---
title: Let Claude prepare UV print files (Windows)
description: Connect Claude to PhotoCraft over MCP and have it fit artwork to a print size, make W1-W4 white and varnish channels, run UV Print Preflight and save the TIFF.
order: 60
verified: 0.16.0
---

PhotoCraft already contains an MCP server: a small program, `photocraft-cli mcp`, that Claude
starts and talks to. Through it Claude can open images, run any PhotoCraft command, look at the
result and save files. This page sets it up on Windows for the flatbed UV workflow: artwork fitted
to a print size, white (W1/W2) and varnish (W3/W4) spot channels, UV Print Preflight and a TIFF
for the RIP.

There are two ways to connect:

| Mode | What you see | Use it when |
|---|---|---|
| **Live** (bridge) | Claude works in the PhotoCraft window you have open, step by step | You want to watch and correct (recommended) |
| **Background** (headless) | No window; Claude opens and saves files in a folder | Batch jobs, or when PhotoCraft isn't open |

Both need the same two programs, built once.

## 1. Build PhotoCraft and the CLI

In PowerShell, in your PhotoCraft folder (the examples use `C:\photocraft`; use yours):

```powershell
cd C:\photocraft
git pull
cargo build --release -p photocraft -p photocraft-cli
```

That makes `C:\photocraft\target\release\photocraft.exe` (the app) and
`C:\photocraft\target\release\photocraft-cli.exe` (the MCP server). Run the build again after every
`git pull`, and restart Claude afterwards.

## 2. Make a jobs folder

Claude may only read and write inside folders you name. Make one for print jobs, without spaces in
the path, for example `C:\PhotoCraftJobs`, and put the customer images there.

## 3a. Live mode: Claude drives your open PhotoCraft

Start PhotoCraft with its control port on. Save this as `C:\photocraft\PhotoCraft for Claude.bat`
and double-click it whenever you want Claude to help:

```bat
@echo off
start "" "C:\photocraft\target\release\photocraft.exe" --control 7878 ^
  --control-token-file "%LOCALAPPDATA%\PhotoCraft\control.token" ^
  --automation-read-root "C:\PhotoCraftJobs" --automation-write-root "C:\PhotoCraftJobs"
```

The first start creates the token file (a password Claude uses to connect). Preferences ›
Integrations › Allow Agent Control must stay on (it is by default).

Then tell Claude where to connect.

**Claude Desktop:** open Settings › Developer › Edit Config. That opens
`%APPDATA%\Claude\claude_desktop_config.json`. Put this in it (replace `YOURNAME` with your
Windows user name; every `\` is written twice in this file), save, and quit and reopen Claude:

```json
{
  "mcpServers": {
    "photocraft": {
      "command": "C:\\photocraft\\target\\release\\photocraft-cli.exe",
      "args": ["mcp", "--bridge", "127.0.0.1:7878",
               "--control-token-file", "C:\\Users\\YOURNAME\\AppData\\Local\\PhotoCraft\\control.token"]
    }
  }
}
```

If the file already has other servers, add `"photocraft": {…}` inside the existing
`"mcpServers"` block instead of making a second one.

**Claude Code** (in a terminal):

```powershell
claude mcp add photocraft -- C:\photocraft\target\release\photocraft-cli.exe mcp --bridge 127.0.0.1:7878 --control-token-file "$env:LOCALAPPDATA\PhotoCraft\control.token"
```

## 3b. Background mode: no window

Use the same config, but with these `args` instead (no token needed):

```json
"args": ["mcp", "--automation-read-root", "C:\\PhotoCraftJobs", "--automation-write-root", "C:\\PhotoCraftJobs"]
```

Claude Code:

```powershell
claude mcp add photocraft -- C:\photocraft\target\release\photocraft-cli.exe mcp --automation-read-root C:\PhotoCraftJobs --automation-write-root C:\PhotoCraftJobs
```

## 4. Check it works

In a new Claude chat, the tools menu should list **photocraft**. Ask:

> Use PhotoCraft to open `customer.png` and tell me its size in pixels and millimetres.

## 5. The UV job, in Claude's words

You can ask in plain language, for example:

> Open `anna.png`. Trim the white margin, fit the artwork into 100 × 160 mm centred on a
> 120 × 180 mm page at 300 ppi. Make W1 white underbase under everything that isn't paper white.
> Put gloss varnish W3 on the black and the red. Run UV Print Preflight, show me a preview, and
> save `anna-print.tif`.

These are the PhotoCraft commands Claude uses for it (all through the `command_run` tool):

| Step | Command | Example params |
|---|---|---|
| Fit to the print size | `image.fitToPrint` | `{"width":120,"height":180,"artWidth":100,"artHeight":160,"trim":"white"}` |
| Select everything that isn't white | `select.colorRange` | `{"color":"#ffffff","fuzziness":20,"invert":true}` |
| Select colours for gloss | `select.colorRange` | `{"colors":["#000000","#c00000"],"fuzziness":40}`, or `{"select":"shadows"}` / `{"select":"reds"}` with `"mode":"add"` |
| Make a white or varnish channel | `channel.uvInk` | `{"name":"W1","source":"selection","choke":1}`, `{"name":"W3","source":"selection"}` |
| Check before printing | `file.uvPreflight` | `{}` |
| See white and gloss on a material | `view.printPreview` | `{}` |
| Save for the RIP | `doc_save` tool | `{"path":"anna-print.tif"}` (spot channels are kept) |

`image.fitToPrint` does Trim, Image Size and Canvas Size in one undo step, so Edit › Undo takes the
whole fit back. Its `width`/`height` are the page and `artWidth`/`artHeight` the box the artwork
must fit inside, all in millimetres.

## When something doesn't work

- **Claude doesn't list photocraft:** the path in `command` is wrong or Claude wasn't fully
  restarted. In PowerShell, run the same path with `mcp --help`; it should print help.
- **"connection refused" in live mode:** PhotoCraft isn't running, or wasn't started with the
  `.bat` file (a normal start doesn't open the control port).
- **"automation filesystem access is not granted" or a path is refused:** the file isn't inside
  `C:\PhotoCraftJobs` (or the folder you named in both places).
- In live mode, `doc_save` takes only `path`; options such as `tiffLayers` need background mode.
