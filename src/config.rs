use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// eclipse | zenith | catppuccin-mocha | cassini
    pub theme: String,
    /// Page opened in new tabs. Empty = the blank keymap tab (no engine spun up).
    pub home: String,
    /// Search URL, `%s` is replaced by the encoded query.
    pub search: String,
    /// UI font family; must be monospace to keep the grid.
    pub font: String,
    /// Master switch for all shield layers.
    pub shield: bool,
    /// What to do with beacons to hosts not on the blocklist: scramble | drop
    pub beacons: String,
    /// Window controls in the bar: auto | always | never
    pub decorations: String,
    /// On start: empty | session | home
    pub start: String,
    /// After closing a tab, focus: neighbour | last
    pub tab_close: String,
    /// Try top-level http:// addresses over https first: upgrade | off
    pub https: String,
    /// On exit, erase cookies, cache, storage: off | everything
    pub clear_on_exit: String,
    /// Media with sound may start on its own: block | allow
    pub autoplay: String,
    /// Not saved: set for ephemeral windows (`--ephemeral`).
    #[serde(skip)]
    pub ephemeral: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: "eclipse".into(),
            home: String::new(),
            search: "https://duckduckgo.com/?q=%s".into(),
            font: if cfg!(windows) { "Consolas".into() } else { "JetBrains Mono".into() },
            shield: true,
            beacons: "scramble".into(),
            decorations: "auto".into(),
            start: "empty".into(),
            tab_close: "neighbour".into(),
            https: "off".into(),
            clear_on_exit: "off".into(),
            autoplay: "block".into(),
            ephemeral: false,
        }
    }
}

pub fn dir() -> PathBuf {
    let d = dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("brxswer");
    let _ = std::fs::create_dir_all(&d);
    d
}

impl Config {
    pub fn path() -> PathBuf {
        dir().join("config.json")
    }

    pub fn load() -> Config {
        match std::fs::read_to_string(Self::path()) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => {
                let c = Config::default();
                c.save();
                c
            }
        }
    }

    pub fn save(&self) {
        if self.ephemeral {
            return;
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Self::path(), s);
        }
    }
}

pub const FILTERS_TEMPLATE: &str = "\
! brxswer user filters — one rule per line, applied on next launch.
!   example.com              block host and subdomains
!   ||example.com^           same, adblock syntax
!   0.0.0.0 example.com      hosts-file syntax
!   example.com/collect      block a path prefix on a host
!   @@||example.com^         never block this host (overrides built-ins)
";
