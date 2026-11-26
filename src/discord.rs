use discord_rich_presence::activity::{Activity, ActivityType, Assets, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Default)]
pub struct PresenceState {
    pub playing: bool,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub artwork_url: String,
    pub position_ms: u64,
    pub duration_ms: u64,
}

#[derive(Clone)]
pub struct DiscordPresence {
    tx: Sender<PresenceCommand>,
}

enum PresenceCommand {
    Update(PresenceState),
    Clear,
    Shutdown,
}

impl DiscordPresence {
    pub fn start(app_id: String) -> Option<Self> {
        let app_id = app_id.trim().to_string();
        if app_id.is_empty() || app_id == "0" {
            return None;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || run_presence(app_id, rx));
        Some(Self { tx })
    }

    pub fn update(&self, state: PresenceState) {
        let _ = self.tx.send(PresenceCommand::Update(state));
    }

    pub fn clear(&self) {
        let _ = self.tx.send(PresenceCommand::Clear);
    }
}

impl Drop for DiscordPresence {
    fn drop(&mut self) {
        let _ = self.tx.send(PresenceCommand::Shutdown);
    }
}

fn discord_log(msg: &str) {
    let path = std::env::temp_dir().join("aurral-desktop-discord.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        use std::io::Write;
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
        let _ = writeln!(f, "[{ts}] {msg}");
    }
}

fn run_presence(app_id: String, rx: Receiver<PresenceCommand>) {
    discord_log(&format!("presence thread started, app_id={app_id}"));
    let mut client: Option<DiscordIpcClient> = None;
    let mut last_state: Option<PresenceState> = None;
    let mut artwork_cache: std::collections::HashMap<String, Option<String>> = std::collections::HashMap::new();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(6)))
        .build()
        .into();

    loop {
        let cmd = match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(cmd) => cmd,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(state) = last_state.clone() {
                    ensure_connected(&app_id, &mut client);
                    let _ = set_presence(&mut client, &state, &mut artwork_cache, &agent);
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => { discord_log("channel disconnected"); break; }
        };

        match cmd {
            PresenceCommand::Shutdown => {
                if let Some(mut c) = client.take() {
                    let _ = c.clear_activity();
                    let _ = c.close();
                }
                break;
            }
            PresenceCommand::Clear => {
                last_state = None;
                if let Some(c) = client.as_mut() {
                    if c.clear_activity().is_err() {
                        client = None;
                    }
                }
            }
            PresenceCommand::Update(state) => {
                discord_log(&format!("update: playing={} title={} artist={} artwork={}", state.playing, state.title, state.artist, state.artwork_url));
                last_state = Some(state.clone());
                ensure_connected(&app_id, &mut client);
                if set_presence(&mut client, &state, &mut artwork_cache, &agent).is_err() {
                    discord_log("set_presence failed, reconnecting");
                    client = None;
                    ensure_connected(&app_id, &mut client);
                    let _ = set_presence(&mut client, &state, &mut artwork_cache, &agent);
                }
            }
        }
    }
}

fn ensure_connected(app_id: &str, client: &mut Option<DiscordIpcClient>) {
    if client.is_some() {
        return;
    }
    let mut c = DiscordIpcClient::new(app_id);
    match c.connect() {
        Ok(_) => { discord_log("connected to Discord IPC"); *client = Some(c); }
        Err(e) => { discord_log(&format!("connect failed: {e:?}")); }
    }
}

fn set_presence(
    client: &mut Option<DiscordIpcClient>,
    state: &PresenceState,
    artwork_cache: &mut std::collections::HashMap<String, Option<String>>,
    agent: &ureq::Agent,
) -> Result<(), ()> {
    let Some(c) = client.as_mut() else {
        return Err(());
    };

    if !state.playing || state.title.trim().is_empty() {
        return c.clear_activity().map_err(|_| ());
    }

    let details = trim_discord(&state.title);
    let artist = state.artist.trim();
    let album = state.album.trim();
    let state_text = match (!artist.is_empty(), !album.is_empty()) {
        (true, true) => trim_discord(&format!("{artist} - {album}")),
        (true, false) => trim_discord(artist),
        (false, true) => trim_discord(album),
        (false, false) => "Listening on Aurral".to_string(),
    };

    let mut activity = Activity::new()
        .activity_type(ActivityType::Listening)
        .details(&details)
        .state(&state_text);

    if state.duration_ms > 0 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let pos = (state.position_ms / 1000) as i64;
        let dur = (state.duration_ms / 1000) as i64;
        let start = now - pos;
        let end = start + dur;
        activity = activity.timestamps(Timestamps::new().start(start).end(end));
    }

    let artwork = resolve_artwork(state, artwork_cache, agent);
    let large_text = trim_discord(if album.is_empty() { &state.title } else { album });
    if let Some(url) = artwork.as_deref() {
        activity = activity.assets(
            Assets::new()
                .large_image(url)
                .large_text(&large_text),
        );
    }

    c.set_activity(activity).map_err(|_| ())
}

fn resolve_artwork(
    state: &PresenceState,
    cache: &mut std::collections::HashMap<String, Option<String>>,
    agent: &ureq::Agent,
) -> Option<String> {
    let raw = state.artwork_url.trim();
    if is_public_artwork_url(raw) {
        return Some(raw.to_string());
    }

    let title = state.title.trim();
    let artist = state.artist.trim();
    if title.is_empty() {
        return None;
    }
    let key = format!("{}\n{}", title.to_lowercase(), artist.to_lowercase());
    if let Some(cached) = cache.get(&key) {
        return cached.clone();
    }

    let found = lookup_itunes_artwork(title, artist, agent);
    if let Some(url) = found.as_deref() {
        discord_log(&format!("artwork lookup hit: {url}"));
    } else {
        discord_log("artwork lookup miss");
    }
    cache.insert(key, found.clone());
    found
}

fn is_public_artwork_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return false;
    }
    let Ok(parsed) = url::Url::parse(url) else { return false; };
    let Some(host) = parsed.host_str() else { return false; };
    let host = host.to_ascii_lowercase();
    if host == "localhost" || host == "::1" || host.ends_with(".local") {
        return false;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return is_public_ip(ip);
    }
    true
}

fn is_public_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let octets = v4.octets();
            let is_cgnat = octets[0] == 100 && (64..=127).contains(&octets[1]); // 100.64.0.0/10
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || is_cgnat)
        }
        std::net::IpAddr::V6(v6) => {
            let segs = v6.segments();
            let is_unique_local = (segs[0] & 0xfe00) == 0xfc00; // fc00::/7
            let is_link_local = (segs[0] & 0xffc0) == 0xfe80; // fe80::/10
            !(v6.is_loopback() || v6.is_unspecified() || is_unique_local || is_link_local)
        }
    }
}

fn lookup_itunes_artwork(title: &str, artist: &str, agent: &ureq::Agent) -> Option<String> {
    let term = if artist.is_empty() { title.to_string() } else { format!("{title} {artist}") };
    let mut query = String::new();
    for b in term.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => query.push(b as char),
            b' ' => query.push('+'),
            _ => { query.push('%'); query.push_str(&format!("{b:02X}")); }
        }
    }
    let url = format!("https://itunes.apple.com/search?term={query}&entity=song&limit=1&media=music");
    let mut response = agent.get(&url).header("Accept", "application/json").call().ok()?;
    let body = response.body_mut().read_to_string().ok()?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    let result = json.get("results")?.as_array()?.first()?;
    let art = result.get("artworkUrl100")?.as_str()?.trim();
    if art.is_empty() { return None; }
    let art = art.replace("100x100bb", "600x600bb");
    if art.starts_with("https://") || art.starts_with("http://") { Some(art) } else { None }
}

fn trim_discord(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        return "Aurral".to_string();
    }
    let mut out: String = s.chars().take(128).collect();
    if out.len() < 2 {
        out.push_str(" ");
    }
    out
}
