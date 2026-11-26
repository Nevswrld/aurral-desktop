#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod discord;

use anyhow::Result;
use config::{normalize_server_url, AppConfig};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::{Icon, WindowBuilder};
use wry::WebViewBuilder;
#[cfg(windows)]
use tao::platform::windows::WindowExtWindows;
#[cfg(windows)]
use wry::WebViewBuilderExtWindows;

#[derive(Debug)]
enum UserEvent {
    Navigate(String),
    ConnectError(String),
    #[cfg(windows)]
    Theme([u8; 3]),
}

fn main() -> Result<()> {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let window_icon = load_icon();
    let mut window_builder = WindowBuilder::new()
        .with_title("Aurral Desktop")
        .with_inner_size(tao::dpi::LogicalSize::new(1280.0, 800.0))
        .with_min_inner_size(tao::dpi::LogicalSize::new(960.0, 640.0));
    if let Some(icon) = window_icon {
        window_builder = window_builder.with_window_icon(Some(icon));
    }
    let window = window_builder.build(&event_loop)?;
    #[cfg(windows)]
    let hwnd = window.hwnd();
    #[cfg(windows)]
    set_title_bar_color(hwnd, [5, 5, 5]);

    let config = Arc::new(Mutex::new(AppConfig::load()));
    let initial_url = config.lock().ok().and_then(|c| c.normalized_server_url());

    let discord = {
        let c = config.lock().ok();
        let enabled = c.as_ref().map(|c| c.discord_enabled).unwrap_or(true);
        let app_id = c
            .as_ref()
            .map(|c| c.discord_app_id.trim().to_string())
            .unwrap_or_default();
        let app_id = if app_id.is_empty() {
            std::env::var("AURRAL_DISCORD_APP_ID").unwrap_or_default()
        } else {
            app_id
        };
        if enabled {
            discord::DiscordPresence::start(app_id)
        } else {
            None
        }
    };

    let handler_config = config.clone();
    let handler_proxy = proxy.clone();
    let handler_discord = discord.clone();
    let mut builder = WebViewBuilder::new()
        .with_devtools(cfg!(debug_assertions))
        .with_autoplay(true)
        .with_initialization_script(format!("{}\n{}", THEME_SCRIPT, PRESENCE_SCRIPT))
        .with_ipc_handler(move |req| {
            let body = req.body().to_string();
            if let Ok(msg) = serde_json::from_str::<serde_json::Value>(&body) {
                match msg.get("type").and_then(|v| v.as_str()) {
                    Some("connect") => {
                        let Some(url) = msg.get("url").and_then(|v| v.as_str()) else {
                            return;
                        };
                        let Some(normalized) = normalize_server_url(url) else {
                            let _ = handler_proxy.send_event(UserEvent::ConnectError(
                                "Enter a valid server URL (for example, http://localhost:3001)."
                                    .to_string(),
                            ));
                            return;
                        };

                        let proxy = handler_proxy.clone();
                        let cfg = handler_config.clone();
                        std::thread::spawn(move || match verify_aurral_server(&normalized) {
                            Ok(()) => {
                                if let Ok(mut c) = cfg.lock() {
                                    c.server_url = normalized.clone();
                                    let _ = c.save();
                                }
                                let _ = proxy.send_event(UserEvent::Navigate(normalized));
                            }
                            Err(err) => {
                                let _ = proxy.send_event(UserEvent::ConnectError(err));
                            }
                        });
                    }
                    Some("disconnect") => {
                        if let Ok(mut c) = handler_config.lock() {
                            c.server_url.clear();
                            let _ = c.save();
                        }
                        let _ = handler_proxy.send_event(UserEvent::Navigate(String::new()));
                    }
                    Some("theme") => {
                        #[cfg(windows)]
                        if let Some(color) = msg.get("color").and_then(|v| v.as_str()) {
                            if let Some(rgb) = parse_hex_color(color) {
                                let _ = handler_proxy.send_event(UserEvent::Theme(rgb));
                            }
                        }
                    }
                    Some("presence") => {
                        if let Some(p) = handler_discord.as_ref() {
                            let playing = msg.get("playing").and_then(|v| v.as_bool()).unwrap_or(false);
                            let title = msg.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            let artist = msg.get("artist").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            let album = msg.get("album").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            let artwork = msg.get("artwork").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            let position_ms = msg.get("positionMs").and_then(|v| v.as_u64()).unwrap_or(0);
                            let duration_ms = msg.get("durationMs").and_then(|v| v.as_u64()).unwrap_or(0);
                            if playing && !title.is_empty() {
                                p.update(discord::PresenceState {
                                    playing,
                                    title,
                                    artist,
                                    album,
                                    artwork_url: artwork,
                                    position_ms,
                                    duration_ms,
                                });
                            } else {
                                p.clear();
                            }
                        }
                    }
                    _ => {}
                }
            }
        });

    #[cfg(windows)]
    {
        builder = builder.with_default_context_menus(false);
    }

    let webview = builder.build(&window)?;

    if let Some(url) = initial_url {
        let _ = webview.load_html(&connect_html(Some(&url)));
        let proxy = proxy.clone();
        std::thread::spawn(move || {
            if verify_aurral_server(&url).is_ok() {
                let _ = proxy.send_event(UserEvent::Navigate(url));
            }
            // If unreachable/invalid, stay on connect page with URL prefilled; no auto-error.
        });
    } else {
        let _ = webview.load_html(&connect_html(None));
    }

    let webview = Arc::new(Mutex::new(webview));
    #[cfg(windows)]
    let event_hwnd = hwnd;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::Exit,
            Event::UserEvent(UserEvent::Navigate(url)) => {
                if let Ok(wv) = webview.lock() {
                    if url.is_empty() {
                        let _ = wv.load_html(&connect_html(None));
                    } else {
                        let _ = wv.load_url(&url);
                    }
                }
            }
            Event::UserEvent(UserEvent::ConnectError(msg)) => {
                if let Ok(wv) = webview.lock() {
                    let js = format!(
                        "window.__aurralSetError && window.__aurralSetError({});",
                        serde_json::to_string(&msg)
                            .unwrap_or_else(|_| "\"Server not found.\"".to_string())
                    );
                    let _ = wv.evaluate_script(&js);
                }
            }
            #[cfg(windows)]
            Event::UserEvent(UserEvent::Theme(rgb)) => {
                set_title_bar_color(event_hwnd, rgb);
            }
            _ => {}
        }
    });
}

fn verify_aurral_server(url: &str) -> std::result::Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|_| "Enter a valid server URL.".to_string())?;
    let mut health_url = parsed.clone();
    let base_path = parsed.path().trim_end_matches('/');
    let health_path = if base_path.is_empty() || base_path == "/" {
        "/api/health".to_string()
    } else {
        format!("{base_path}/api/health")
    };
    health_url.set_path(&health_path);
    health_url.set_query(None);
    health_url.set_fragment(None);

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .build()
        .into();

    let mut response = agent
        .get(health_url.as_str())
        .header("Accept", "application/json")
        .call()
        .map_err(|_| {
            "Server not found. Check the URL and make sure your Aurral server is running."
                .to_string()
        })?;

    let body = response.body_mut().read_to_string().map_err(|_| {
        "Server responded, but it doesn't look like an Aurral server.".to_string()
    })?;

    let json: serde_json::Value = serde_json::from_str(&body).map_err(|_| {
        "Server responded, but it doesn't look like an Aurral server.".to_string()
    })?;

    let looks_like_aurral = json.get("appVersion").is_some()
        || json.get("apiContract").is_some()
        || json
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.eq_ignore_ascii_case("aurral"))
            .unwrap_or(false);

    if looks_like_aurral {
        Ok(())
    } else {
        Err("Server responded, but it doesn't look like an Aurral server.".to_string())
    }
}

fn html_escape_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn connect_html(prefill_url: Option<&str>) -> String {
    let logo: &str = include_str!("../assets/arralogo.svg");
    let prefill = prefill_url.map(html_escape_attr).unwrap_or_default();
    let mut html = String::from(r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>Connect to Aurral</title>
  <style>
    :root { color-scheme: dark; }
    * { box-sizing: border-box; }
    body {
      margin: 0; min-height: 100vh; display: grid; place-items: center;
      background: #050505; color: #fff; font-family: Inter, ui-sans-serif, system-ui, -apple-system, sans-serif;
    }
    .card {
      width: min(440px, calc(100vw - 32px)); background: #000; border: 1px solid rgba(255,255,255,.08);
      border-radius: 14px; padding: 28px;
    }
    .brand { display: flex; align-items: center; gap: 12px; margin-bottom: 20px; }
    .brand-logo { width: 40px; height: 40px; display: grid; place-items: center; }
    .brand-logo svg { width: 40px; height: 40px; display: block; }
    h1 { margin: 0; font-size: 32px; letter-spacing: -0.02em; line-height: 1; }
    p { margin: 0 0 24px; color: #b3b3b3; font-size: 14px; }
    label { display: block; margin: 0 0 8px; color: #b3b3b3; font-size: 13px; }
    input {
      width: 100%; padding: 12px 14px; border-radius: 8px; border: 1px solid rgba(255,255,255,.14);
      background: #0e0e0e; color: #fff; font: inherit; outline: none;
    }
    input:focus { border-color: rgba(255,255,255,.35); }
    button {
      width: 100%; margin-top: 14px; padding: 12px 14px; border-radius: 8px; border: 0;
      background: #fff; color: #000; font: inherit; font-weight: 600; cursor: pointer;
    }
    button:hover { background: #e5e5e5; }
    button:disabled { opacity: .6; cursor: default; }
    .error { margin-top: 10px; color: #ef4444; font-size: 13px; min-height: 1.2em; }
  </style>
</head>
<body>
  <main class="card">
    <div class="brand">
      <div class="brand-logo">__LOGO__</div>
      <h1>Aurral</h1>
    </div>
    <p>Connect to your Aurral server.</p>
    <form id="form">
      <label for="url">Server URL</label>
      <input id="url" name="url" placeholder="http://localhost:3001" autocomplete="off" spellcheck="false" value="__PREFILL__" />
      <div class="error" id="error"></div>
      <button id="submit" type="submit">Connect</button>
    </form>
  </main>
  <script>
    const form = document.getElementById('form');
    const input = document.getElementById('url');
    const error = document.getElementById('error');
    const submit = document.getElementById('submit');
    window.__aurralSetError = (msg) => {
      error.textContent = msg || '';
      submit.disabled = false;
      submit.textContent = 'Connect';
    };
    form.addEventListener('submit', (e) => {
      e.preventDefault();
      const url = input.value.trim();
      if (!url) { error.textContent = 'Enter your Aurral server URL.'; return; }
      error.textContent = '';
      submit.disabled = true;
      submit.textContent = 'Connecting...';
      window.ipc.postMessage(JSON.stringify({ type: 'connect', url }));
    });
  </script>
</body>
</html>
"##);
    html = html.replace("__LOGO__", logo);
    html = html.replace("__PREFILL__", &prefill);
    html
}

fn load_icon() -> Option<Icon> {
    let bytes = include_bytes!("../assets/aurral-icon-512.png");
    let img = image::load_from_memory(bytes).ok()?.into_rgba8();
    let (width, height) = img.dimensions();
    Icon::from_rgba(img.into_raw(), width, height).ok()
}

fn parse_hex_color(value: &str) -> Option<[u8; 3]> {
    let hex = value.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some([r, g, b])
}

#[cfg(windows)]
fn set_title_bar_color(hwnd: isize, rgb: [u8; 3]) {
    use std::ffi::c_void;
    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(hwnd: isize, attr: u32, pv: *const c_void, cb: u32) -> i32;
    }
    #[link(name = "user32")]
    extern "system" {
        fn SetWindowPos(hwnd: isize, insert_after: isize, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
    }
    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    const DWMWA_CAPTION_COLOR: u32 = 35;
    const DWMWA_TEXT_COLOR: u32 = 36;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOZORDER: u32 = 0x0004;
    const SWP_FRAMECHANGED: u32 = 0x0020;
    let luminance = (0.299 * rgb[0] as f32) + (0.587 * rgb[1] as f32) + (0.114 * rgb[2] as f32);
    let dark: i32 = if luminance > 140.0 { 0 } else { 1 };
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const _ as *const c_void,
            std::mem::size_of::<i32>() as u32,
        );
        let colorref: u32 = (rgb[0] as u32) | ((rgb[1] as u32) << 8) | ((rgb[2] as u32) << 16);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR,
            &colorref as *const _ as *const c_void,
            std::mem::size_of::<u32>() as u32,
        );
        let text: u32 = if luminance > 140.0 { 0x00000000 } else { 0x00FFFFFF };
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_TEXT_COLOR,
            &text as *const _ as *const c_void,
            std::mem::size_of::<u32>() as u32,
        );
        let _ = SetWindowPos(hwnd, 0, 0, 0, 0, 0, SWP_NOSIZE | SWP_NOMOVE | SWP_NOZORDER | SWP_FRAMECHANGED);
    }
}

const PRESENCE_SCRIPT: &str = r#"
(() => {
  let last = '';
  const send = (payload) => {
    try {
      const key = JSON.stringify(payload);
      if (key === last) return;
      last = key;
      if (window.ipc) window.ipc.postMessage(JSON.stringify(payload));
    } catch (_) {}
  };
  const read = () => {
    try {
      const ms = navigator.mediaSession;
      const md = ms && ms.metadata;
      const audio = document.querySelector('audio,video');
      const playing = ms ? ms.playbackState === 'playing' : (audio ? !audio.paused : false);
      const title = md?.title || '';
      const artist = md?.artist || '';
      const album = md?.album || '';
      let artwork = '';
      if (md?.artwork && md.artwork.length) artwork = md.artwork[md.artwork.length - 1]?.src || md.artwork[0]?.src || '';
      const durationMs = audio && isFinite(audio.duration) ? Math.max(0, Math.round(audio.duration * 1000)) : 0;
      const positionMs = audio && isFinite(audio.currentTime) ? Math.max(0, Math.round(audio.currentTime * 1000)) : 0;
      send({ type: 'presence', playing: !!playing && !!title, title, artist, album, artwork, positionMs, durationMs });
    } catch (_) {}
  };
  const tick = () => { read(); };
  setInterval(tick, 5000);
  document.addEventListener('play', tick, true);
  document.addEventListener('pause', tick, true);
  document.addEventListener('loadedmetadata', tick, true);
  window.addEventListener('load', () => { tick(); setTimeout(tick, 1000); setTimeout(tick, 3000); });
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', tick, { once: true });
  else tick();
})();
"#;
const THEME_SCRIPT: &str = r#"
(() => {
  let last = '';
  const toHex = (bg) => {
    const m = String(bg || '').match(/rgba?\(([\d.]+),\s*([\d.]+),\s*([\d.]+)/i);
    if (!m) return '';
    const hex = (n) => Number(n).toString(16).padStart(2, '0');
    return '#' + hex(Math.round(m[1])) + hex(Math.round(m[2])) + hex(Math.round(m[3]));
  };
  const post = () => {
    try {
      let color = '';
      const candidates = ['.app-topbar', '.sidebar-brand', 'header', 'nav', 'body', 'html'];
      for (const sel of candidates) {
        const el = document.querySelector(sel);
        if (!el) continue;
        const bg = getComputedStyle(el).backgroundColor;
        const m = bg.match(/rgba?\(([\d.]+),\s*([\d.]+),\s*([\d.]+)(?:,\s*([\d.]+))?/i);
        if (!m) continue;
        const a = m[4] === undefined ? 1 : Number(m[4]);
        if (a < 0.95) continue;
        color = toHex(bg);
        if (/^#([0-9a-f]{6})$/i.test(color)) break;
      }
      if (!/^#([0-9a-f]{6})$/i.test(color)) {
        const meta = document.querySelector('meta[name="theme-color"]:not([media])');
        if (meta && /^#([0-9a-f]{6})$/i.test(meta.content || '')) color = meta.content.trim();
      }
      if (/^#([0-9a-f]{6})$/i.test(color) && color !== last && window.ipc) {
        last = color;
        window.ipc.postMessage(JSON.stringify({ type: 'theme', color }));
      }
    } catch (_) {}
  };
  const run = () => { post(); setTimeout(post, 250); setTimeout(post, 1000); setTimeout(post, 2500); };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', run, { once: true });
  else run();
  new MutationObserver(run).observe(document.documentElement, { subtree: true, childList: true, attributes: true });
  window.addEventListener('load', run);
  window.addEventListener('focus', post);
  document.addEventListener('visibilitychange', () => { if (!document.hidden) post(); });
  window.addEventListener('storage', post);
  setInterval(post, 1500);
})();
"#;
