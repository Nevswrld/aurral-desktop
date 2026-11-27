use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CloseBehavior {
    Close,
    MinimizeToTray,
}

impl Default for CloseBehavior {
    fn default() -> Self {
        Self::Close
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub server_url: String,
    #[serde(default = "default_discord_enabled")]
    pub discord_enabled: bool,
    #[serde(default)]
    pub discord_app_id: String,
    #[serde(default)]
    pub close_behavior: CloseBehavior,
    #[serde(default)]
    pub main_window: Option<WindowState>,
    #[serde(default)]
    pub settings_window: Option<WindowState>,
}

fn default_discord_enabled() -> bool {
    true
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server_url: String::new(),
            discord_enabled: true,
            discord_app_id: String::new(),
            close_behavior: CloseBehavior::default(),
            main_window: None,
            settings_window: None,
        }
    }
}

impl AppConfig {
    pub fn path() -> Result<PathBuf> {
        let dirs = ProjectDirs::from("org", "aurral", "AurralDesktop")
            .context("Could not determine config directory")?;
        let dir = dirs.config_dir();
        fs::create_dir_all(dir).context("Could not create config directory")?;
        Ok(dir.join("config.json"))
    }

    pub fn load() -> Self {
        let Ok(path) = Self::path() else {
            return Self::default();
        };
        let Ok(data) = fs::read_to_string(path) else {
            return Self::default();
        };
        serde_json::from_str(&data).unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        let data = serde_json::to_string_pretty(self).context("Could not serialize config")?;
        fs::write(path, data).context("Could not write config")?;
        Ok(())
    }

    pub fn normalized_server_url(&self) -> Option<String> {
        normalize_server_url(&self.server_url)
    }
}

pub fn normalize_server_url(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };
    let mut parsed = url::Url::parse(&with_scheme).ok()?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return None;
    }
    parsed.set_fragment(None);
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    // keep path if user entered one, but trim trailing slash for consistency
    let path = parsed.path().trim_end_matches('/').to_string();
    parsed.set_path(if path.is_empty() { "/" } else { &path });
    Some(parsed.to_string())
}
