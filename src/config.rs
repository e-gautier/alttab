use serde::Deserialize;
use std::path::PathBuf;

/// Top-level configuration.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(default)]
pub struct Config {
    pub colors: Colors,
    pub layout: Layout,
}

/// Color configuration. All colors are specified as hex strings: "#RRGGBB" or "#RRGGBBAA".
#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct Colors {
    /// Overlay background color.
    pub background: String,
    /// Normal item background color.
    pub item: String,
    /// Selected item background color.
    pub selected: String,
    /// Title text color.
    pub title: String,
    /// App ID (subtitle) text color.
    pub app_id: String,
}

/// Layout configuration.
#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct Layout {
    /// Overlay width in pixels.
    pub width: u32,
    /// Maximum overlay height in pixels.
    pub max_height: u32,
    /// Height of each window item in pixels.
    pub item_height: u32,
    /// Spacing between items in pixels.
    pub item_spacing: u32,
    /// Padding around the overlay content in pixels.
    pub padding: u32,
    /// Corner radius for the overlay background.
    pub corner_radius: f32,
    /// Corner radius for individual items.
    pub item_corner_radius: f32,
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            background: "#1E1E1EE6".to_string(),
            item: "#323232".to_string(),
            selected: "#3C78D8".to_string(),
            title: "#FFFFFF".to_string(),
            app_id: "#AAAAAA".to_string(),
        }
    }
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            width: 500,
            max_height: 600,
            item_height: 48,
            item_spacing: 8,
            padding: 16,
            corner_radius: 12.0,
            item_corner_radius: 8.0,
        }
    }
}

/// Parse a hex color string like "#RRGGBB" or "#RRGGBBAA" into (r, g, b, a).
pub fn parse_hex_color(s: &str) -> Option<(u8, u8, u8, u8)> {
    let s = s.strip_prefix('#').unwrap_or(s);
    match s.len() {
        6 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            Some((r, g, b, 255))
        }
        8 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            let a = u8::from_str_radix(&s[6..8], 16).ok()?;
            Some((r, g, b, a))
        }
        _ => None,
    }
}

/// Get the config file path.
fn config_path() -> PathBuf {
    let config_dir = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".config")
        });
    config_dir.join("alttab").join("config.toml")
}

/// Load configuration from the config file, falling back to defaults.
pub fn load_config() -> Config {
    let path = config_path();
    if !path.exists() {
        log::info!("No config file at {:?}, using defaults", path);
        return Config::default();
    }

    match std::fs::read_to_string(&path) {
        Ok(contents) => match toml::from_str::<Config>(&contents) {
            Ok(config) => {
                log::info!("Loaded config from {:?}", path);
                config
            }
            Err(e) => {
                log::warn!(
                    "Failed to parse config at {:?}: {}, using defaults",
                    path,
                    e
                );
                Config::default()
            }
        },
        Err(e) => {
            log::warn!("Failed to read config at {:?}: {}, using defaults", path, e);
            Config::default()
        }
    }
}
