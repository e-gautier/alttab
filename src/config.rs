use serde::Deserialize;
use std::path::PathBuf;

/// Parsed RGBA color tuple.
pub type Rgba = (u8, u8, u8, u8);

/// Top-level configuration.
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub colors: ResolvedColors,
    pub layout: Layout,
}

/// Resolved color values (parsed from hex strings at load time).
#[derive(Debug, Clone)]
pub struct ResolvedColors {
    pub background: Rgba,
    pub item: Rgba,
    pub selected: Rgba,
    pub title: Rgba,
    pub app_id: Rgba,
}

impl Default for ResolvedColors {
    fn default() -> Self {
        Self {
            background: (30, 30, 30, 230),
            item: (50, 50, 50, 255),
            selected: (60, 120, 216, 255),
            title: (255, 255, 255, 255),
            app_id: (170, 170, 170, 255),
        }
    }
}

/// Raw color configuration as deserialized from TOML.
/// All colors are specified as hex strings: "#RRGGBB" or "#RRGGBBAA".
#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
struct RawColors {
    background: String,
    item: String,
    selected: String,
    title: String,
    app_id: String,
}

/// Raw top-level config for deserialization.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(default)]
struct RawConfig {
    colors: RawColors,
    layout: Layout,
}

impl Default for RawColors {
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
pub fn parse_hex_color(s: &str) -> Option<Rgba> {
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

/// Resolve raw hex color strings into RGBA tuples, using defaults for invalid values.
fn resolve_colors(raw: &RawColors) -> ResolvedColors {
    let defaults = ResolvedColors::default();
    ResolvedColors {
        background: parse_hex_color(&raw.background).unwrap_or(defaults.background),
        item: parse_hex_color(&raw.item).unwrap_or(defaults.item),
        selected: parse_hex_color(&raw.selected).unwrap_or(defaults.selected),
        title: parse_hex_color(&raw.title).unwrap_or(defaults.title),
        app_id: parse_hex_color(&raw.app_id).unwrap_or(defaults.app_id),
    }
}

/// Validate and clamp layout values to sane ranges.
fn validate_layout(layout: &mut Layout) {
    let defaults = Layout::default();

    if layout.width < 100 {
        log::warn!(
            "Layout width {} too small, using default {}",
            layout.width,
            defaults.width
        );
        layout.width = defaults.width;
    }
    if layout.max_height < 100 {
        log::warn!(
            "Layout max_height {} too small, using default {}",
            layout.max_height,
            defaults.max_height
        );
        layout.max_height = defaults.max_height;
    }
    if layout.item_height < 16 {
        log::warn!(
            "Layout item_height {} too small, using default {}",
            layout.item_height,
            defaults.item_height
        );
        layout.item_height = defaults.item_height;
    }
    if layout.corner_radius < 0.0 {
        log::warn!(
            "Layout corner_radius {} negative, using 0.0",
            layout.corner_radius
        );
        layout.corner_radius = 0.0;
    }
    if layout.item_corner_radius < 0.0 {
        log::warn!(
            "Layout item_corner_radius {} negative, using 0.0",
            layout.item_corner_radius
        );
        layout.item_corner_radius = 0.0;
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
        Ok(contents) => match toml::from_str::<RawConfig>(&contents) {
            Ok(raw) => {
                log::info!("Loaded config from {:?}", path);
                let colors = resolve_colors(&raw.colors);
                let mut layout = raw.layout;
                validate_layout(&mut layout);
                Config { colors, layout }
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
