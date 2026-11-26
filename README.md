# Aurral Desktop

A lightweight Windows desktop client for [Aurral](https://github.com/lklynet/aurral), built in Rust.

This app is a thin WebView2 wrapper around your Aurral server's web frontend, so the desktop experience looks and behaves like the Aurral web app.

## Features

- Native Windows desktop window for Aurral
- Connect to any Aurral server by URL
- Saves your server URL locally for next launch
- Uses the real Aurral web frontend for login, browsing, and playback

## Requirements

- Windows 10/11
- Microsoft Edge WebView2 Runtime (usually already installed on Windows 10/11)
- A running Aurral server
- Rust stable toolchain for building from source

## Build

```powershell
cargo build --release
```

The executable will be created at:

```text
target\release\aurral-desktop.exe
```

## Run

```powershell
cargo run --release
```

Or run the built `aurral-desktop.exe` directly.

## Usage

1. Start the app.
2. Enter your Aurral server URL, for example `http://localhost:3001`.
3. Click **Connect**.
4. Sign in and use Aurral as you would in the browser.

The selected server URL is stored locally in your user config directory.

## Notes

- The desktop app renders the Aurral web frontend inside WebView2 for visual parity with the web app.
- Authentication and playback are handled by Aurral's web frontend/server.
- To change servers, clear the saved config in your user config directory or reinstall with a fresh config.

## Aurral

- Docs: https://docs.aurral.org/api/overview/
- Source: https://github.com/lklynet/aurral
