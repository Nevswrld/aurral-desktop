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

fn run_presence(app_id: String, rx: Receiver<PresenceCommand>) {
    let mut client: Option<DiscordIpcClient> = None;
    let mut last_state: Option<PresenceState> = None;

    loop {
        let cmd = match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(cmd) => cmd,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(state) = last_state.clone() {
                    ensure_connected(&app_id, &mut client);
                    let _ = set_presence(&mut client, &state);
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
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
                last_state = Some(state.clone());
                ensure_connected(&app_id, &mut client);
                if set_presence(&mut client, &state).is_err() {
                    client = None;
                    ensure_connected(&app_id, &mut client);
                    let _ = set_presence(&mut client, &state);
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
    if c.connect().is_ok() {
        *client = Some(c);
    }
}

fn set_presence(client: &mut Option<DiscordIpcClient>, state: &PresenceState) -> Result<(), ()> {
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
        (true, true) => trim_discord(&format!("{artist} • {album}")),
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

    let artwork = state.artwork_url.trim();
    let large_text = trim_discord(if album.is_empty() { &state.title } else { album });
    if artwork.starts_with("http://") || artwork.starts_with("https://") {
        activity = activity.assets(
            Assets::new()
                .large_image(artwork)
                .large_text(&large_text),
        );
    }

    c.set_activity(activity).map_err(|_| ())
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