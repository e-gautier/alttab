use fontdue::{Font, FontSettings};
use std::collections::HashMap;

/// Embedded DroidSans font (Apache 2.0 license).
const FONT_DATA: &[u8] = include_bytes!("../fonts/DroidSans.ttf");

/// Cached rasterized glyph.
struct RasterizedGlyph {
    bitmap: Vec<u8>,
    width: usize,
    height: usize,
    /// Horizontal offset from the pen position to the left edge of the glyph bitmap.
    x_offset: i32,
    /// Vertical offset from the baseline to the top of the glyph bitmap.
    y_offset: i32,
    /// How far to advance the pen horizontally after this glyph.
    advance_x: f32,
}

/// Font renderer with glyph caching.
pub struct FontRenderer {
    font: Font,
    /// Cache keyed by (char, font_size_in_tenths_of_px) to avoid float keys.
    cache: HashMap<(char, u32), RasterizedGlyph>,
}

impl FontRenderer {
    pub fn new() -> Self {
        let font = Font::from_bytes(
            FONT_DATA,
            FontSettings {
                scale: 40.0, // default scale hint, actual size passed per-rasterize
                ..FontSettings::default()
            },
        )
        .expect("Failed to load embedded font");

        Self {
            font,
            cache: HashMap::new(),
        }
    }

    /// Rasterize a glyph at the given pixel size, caching the result.
    fn get_glyph(&mut self, ch: char, size_px: f32) -> &RasterizedGlyph {
        let key = (ch, (size_px * 10.0) as u32);
        if !self.cache.contains_key(&key) {
            let (metrics, bitmap) = self.font.rasterize(ch, size_px);
            let glyph = RasterizedGlyph {
                bitmap,
                width: metrics.width,
                height: metrics.height,
                x_offset: metrics.xmin,
                y_offset: metrics.ymin,
                advance_x: metrics.advance_width,
            };
            self.cache.insert(key, glyph);
        }
        self.cache.get(&key).unwrap()
    }

    /// Draw text into an ARGB8888 (BGRA in memory, little-endian) pixel buffer.
    ///
    /// - `data`: the raw pixel buffer (BGRA byte order)
    /// - `canvas_w`, `canvas_h`: dimensions of the buffer
    /// - `x`, `y`: top-left position of the text baseline area
    /// - `text`: the string to render
    /// - `color`: packed ARGB u32 (same format as the rest of the renderer)
    /// - `size_px`: font size in pixels
    pub fn draw_text(
        &mut self,
        data: &mut [u8],
        canvas_w: u32,
        canvas_h: u32,
        x: u32,
        y: u32,
        text: &str,
        color: u32,
        size_px: f32,
    ) {
        let [ca, cr, cg, cb] = color.to_be_bytes();
        let mut pen_x = x as f32;

        // Use the font's metrics to compute a consistent baseline offset.
        // y is the top of the text area; baseline is offset by ascent.
        let line_metrics = self.font.horizontal_line_metrics(size_px);
        let ascent = line_metrics.map(|m| m.ascent).unwrap_or(size_px * 0.8);

        for ch in text.chars() {
            let glyph = self.get_glyph(ch, size_px);
            let gw = glyph.width;
            let gh = glyph.height;
            let gx_offset = glyph.x_offset;
            let gy_offset = glyph.y_offset;
            let advance = glyph.advance_x;

            // Glyph top-left in canvas coordinates:
            // pen_x + x_offset = left edge
            // y + ascent - (height + y_offset) = top edge
            // (y_offset is from baseline to bottom of glyph bitmap in fontdue)
            let draw_x = pen_x + gx_offset as f32;
            let draw_y = y as f32 + ascent - gh as f32 - gy_offset as f32;

            for row in 0..gh {
                let py = draw_y as i32 + row as i32;
                if py < 0 || py >= canvas_h as i32 {
                    continue;
                }
                let py = py as u32;

                for col in 0..gw {
                    let px = draw_x as i32 + col as i32;
                    if px < 0 || px >= canvas_w as i32 {
                        continue;
                    }
                    let px = px as u32;

                    let coverage = glyph.bitmap[row * gw + col] as u32;
                    if coverage == 0 {
                        continue;
                    }

                    let idx = ((py * canvas_w + px) * 4) as usize;
                    if idx + 3 >= data.len() {
                        continue;
                    }

                    // Effective alpha = font color alpha * glyph coverage
                    let sa = (ca as u32 * coverage + 128) / 255;

                    if sa >= 255 {
                        data[idx] = cb;
                        data[idx + 1] = cg;
                        data[idx + 2] = cr;
                        data[idx + 3] = 255;
                    } else if sa > 0 {
                        let db = data[idx] as u32;
                        let dg = data[idx + 1] as u32;
                        let dr = data[idx + 2] as u32;
                        let da = data[idx + 3] as u32;

                        let inv_sa = 255 - sa;
                        let out_b = (cb as u32 * sa + db * inv_sa + 128) / 255;
                        let out_g = (cg as u32 * sa + dg * inv_sa + 128) / 255;
                        let out_r = (cr as u32 * sa + dr * inv_sa + 128) / 255;
                        let out_a = (sa * 255 + da * inv_sa + 128) / 255;

                        data[idx] = out_b.min(255) as u8;
                        data[idx + 1] = out_g.min(255) as u8;
                        data[idx + 2] = out_r.min(255) as u8;
                        data[idx + 3] = out_a.min(255) as u8;
                    }
                }
            }

            pen_x += advance;
        }
    }

    /// Measure the width of a string at the given pixel size.
    pub fn measure_text(&mut self, text: &str, size_px: f32) -> f32 {
        let mut width = 0.0f32;
        for ch in text.chars() {
            let glyph = self.get_glyph(ch, size_px);
            width += glyph.advance_x;
        }
        width
    }
}
