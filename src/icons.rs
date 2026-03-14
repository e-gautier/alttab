use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// RGBA pixel data for a loaded icon.
#[derive(Clone)]
pub struct IconData {
    pub width: u32,
    pub height: u32,
    /// RGBA pixel data, row-major.
    pub pixels: Vec<u8>,
}

/// Cache of loaded icons, keyed by app_id.
pub struct IconCache {
    cache: HashMap<String, Option<IconData>>,
    desktop_dirs: Vec<PathBuf>,
    icon_dirs: Vec<PathBuf>,
}

impl IconCache {
    pub fn new() -> Self {
        let desktop_dirs = Self::find_desktop_dirs();
        let icon_dirs = Self::find_icon_dirs();
        Self {
            cache: HashMap::new(),
            desktop_dirs,
            icon_dirs,
        }
    }

    fn find_desktop_dirs() -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        // User local
        if let Ok(data_home) = std::env::var("XDG_DATA_HOME") {
            dirs.push(PathBuf::from(data_home).join("applications"));
        } else if let Ok(home) = std::env::var("HOME") {
            dirs.push(PathBuf::from(home).join(".local/share/applications"));
        }

        // System dirs from XDG_DATA_DIRS
        let data_dirs = std::env::var("XDG_DATA_DIRS")
            .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
        for dir in data_dirs.split(':') {
            if !dir.is_empty() {
                dirs.push(PathBuf::from(dir).join("applications"));
            }
        }

        dirs
    }

    fn find_icon_dirs() -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        // Preferred sizes for our use case (we render at ~32x32)
        // Search larger sizes first (we'll scale down)
        let sizes = [
            "48x48", "64x64", "32x32", "128x128", "256x256", "24x24", "scalable",
        ];

        // User local
        if let Ok(data_home) = std::env::var("XDG_DATA_HOME") {
            for size in &sizes {
                dirs.push(
                    PathBuf::from(&data_home)
                        .join("icons/hicolor")
                        .join(size)
                        .join("apps"),
                );
            }
        } else if let Ok(home) = std::env::var("HOME") {
            for size in &sizes {
                dirs.push(
                    PathBuf::from(&home)
                        .join(".local/share/icons/hicolor")
                        .join(size)
                        .join("apps"),
                );
            }
        }

        // System hicolor theme
        for size in &sizes {
            dirs.push(
                PathBuf::from("/usr/share/icons/hicolor")
                    .join(size)
                    .join("apps"),
            );
        }

        // Pixmaps fallback
        dirs.push(PathBuf::from("/usr/share/pixmaps"));

        dirs
    }

    /// Get the icon for an app_id. Returns None if no icon found.
    /// Results are cached.
    pub fn get(&mut self, app_id: &str) -> Option<&IconData> {
        if !self.cache.contains_key(app_id) {
            let icon = self.load_icon(app_id);
            self.cache.insert(app_id.to_string(), icon);
        }
        self.cache.get(app_id).and_then(|o| o.as_ref())
    }

    /// Read-only lookup of a previously cached icon (does not trigger loading).
    pub fn peek(&self, app_id: &str) -> Option<&IconData> {
        self.cache.get(app_id).and_then(|o| o.as_ref())
    }

    fn load_icon(&self, app_id: &str) -> Option<IconData> {
        // Step 1: Find the icon name from .desktop file
        let icon_name = self.find_icon_name(app_id)?;

        // Step 2: If it's an absolute path, load directly
        if icon_name.starts_with('/') {
            return self.load_png(Path::new(&icon_name));
        }

        // Step 3: Search icon directories for the icon name
        for dir in &self.icon_dirs {
            // Skip scalable directory (we only handle PNG)
            if dir.to_str().map_or(false, |s| s.contains("scalable")) {
                continue;
            }
            let path = dir.join(format!("{}.png", icon_name));
            if path.exists() {
                if let Some(icon) = self.load_png(&path) {
                    return Some(icon);
                }
            }
        }

        None
    }

    fn find_icon_name(&self, app_id: &str) -> Option<String> {
        // Try direct match: app_id.desktop
        for dir in &self.desktop_dirs {
            let path = dir.join(format!("{}.desktop", app_id));
            if let Some(name) = Self::read_icon_from_desktop(&path) {
                return Some(name);
            }
        }

        // Try case-insensitive search and StartupWMClass match
        let app_id_lower = app_id.to_lowercase();
        for dir in &self.desktop_dirs {
            let entries = match fs::read_dir(dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.extension().map_or(false, |e| e == "desktop") {
                    continue;
                }

                // Check filename match (case-insensitive)
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if stem.to_lowercase() == app_id_lower {
                        if let Some(name) = Self::read_icon_from_desktop(&path) {
                            return Some(name);
                        }
                    }
                }

                // Check StartupWMClass
                if let Ok(contents) = fs::read_to_string(&path) {
                    let mut has_matching_wm_class = false;
                    let mut icon_name = None;
                    for line in contents.lines() {
                        if let Some(val) = line.strip_prefix("StartupWMClass=") {
                            if val.trim().eq_ignore_ascii_case(app_id) {
                                has_matching_wm_class = true;
                            }
                        }
                        if let Some(val) = line.strip_prefix("Icon=") {
                            icon_name = Some(val.trim().to_string());
                        }
                    }
                    if has_matching_wm_class {
                        if let Some(name) = icon_name {
                            return Some(name);
                        }
                    }
                }
            }
        }

        // Last resort: try the app_id itself as the icon name
        Some(app_id.to_string())
    }

    fn read_icon_from_desktop(path: &Path) -> Option<String> {
        let contents = fs::read_to_string(path).ok()?;
        for line in contents.lines() {
            if let Some(val) = line.strip_prefix("Icon=") {
                let val = val.trim();
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
        None
    }

    fn load_png(&self, path: &Path) -> Option<IconData> {
        let file = fs::File::open(path).ok()?;
        let decoder = png::Decoder::new(file);
        let mut reader = decoder.read_info().ok()?;

        let mut buf = vec![0u8; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).ok()?;

        let width = info.width;
        let height = info.height;

        // Convert to RGBA
        let pixels = match info.color_type {
            png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
            png::ColorType::Rgb => {
                let src = &buf[..info.buffer_size()];
                let mut rgba = Vec::with_capacity((width * height * 4) as usize);
                for chunk in src.chunks_exact(3) {
                    rgba.push(chunk[0]);
                    rgba.push(chunk[1]);
                    rgba.push(chunk[2]);
                    rgba.push(255);
                }
                rgba
            }
            png::ColorType::GrayscaleAlpha => {
                let src = &buf[..info.buffer_size()];
                let mut rgba = Vec::with_capacity((width * height * 4) as usize);
                for chunk in src.chunks_exact(2) {
                    rgba.push(chunk[0]);
                    rgba.push(chunk[0]);
                    rgba.push(chunk[0]);
                    rgba.push(chunk[1]);
                }
                rgba
            }
            png::ColorType::Grayscale => {
                let src = &buf[..info.buffer_size()];
                let mut rgba = Vec::with_capacity((width * height * 4) as usize);
                for &g in src {
                    rgba.push(g);
                    rgba.push(g);
                    rgba.push(g);
                    rgba.push(255);
                }
                rgba
            }
            _ => {
                log::debug!(
                    "Unsupported PNG color type {:?} for {:?}",
                    info.color_type,
                    path
                );
                return None;
            }
        };

        log::debug!("Loaded icon {:?} ({}x{})", path, width, height);
        Some(IconData {
            width,
            height,
            pixels,
        })
    }
}
