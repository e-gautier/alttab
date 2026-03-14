use tiny_skia::{Color, FillRule, Paint, PathBuilder, PixmapMut, Transform};

use crate::config::{parse_hex_color, Config};
use crate::font::FontRenderer;
use crate::icons::IconData;
use crate::toplevel::ToplevelInfo;

const TEXT_LEFT_MARGIN: u32 = 16;
const ICON_SIZE: u32 = 32;
const ICON_LEFT_MARGIN: u32 = 8;
const ICON_TEXT_GAP: u32 = 8;

/// Font sizes in pixels.
const TITLE_FONT_SIZE: f32 = 14.0;
const APPID_FONT_SIZE: f32 = 11.0;

/// Calculate overlay dimensions based on number of windows and config.
pub fn calc_overlay_size(num_windows: usize, config: &Config) -> (u32, u32) {
    let width = config.layout.width;
    let padding = config.layout.padding;
    let item_h = config.layout.item_height;
    let item_sp = config.layout.item_spacing;
    let height = padding * 2 + (num_windows as u32) * (item_h + item_sp) - item_sp;
    let height = height.max(80).min(config.layout.max_height);
    (width, height)
}

fn color_from_hex(hex: &str, fallback: (u8, u8, u8, u8)) -> Color {
    let (r, g, b, a) = parse_hex_color(hex).unwrap_or(fallback);
    Color::from_rgba8(r, g, b, a)
}

fn argb_from_hex(hex: &str, fallback: (u8, u8, u8, u8)) -> u32 {
    let (r, g, b, a) = parse_hex_color(hex).unwrap_or(fallback);
    (a as u32) << 24 | (r as u32) << 16 | (g as u32) << 8 | (b as u32)
}

/// Render the overlay into an ARGB8888 pixel buffer.
pub fn render_overlay(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    windows: &[&ToplevelInfo],
    icons: &[Option<&IconData>],
    selected: usize,
    config: &Config,
    font: &mut FontRenderer,
) {
    let Some(mut pixmap) = PixmapMut::from_bytes(canvas, width, height) else {
        return;
    };

    let bg = color_from_hex(&config.colors.background, (30, 30, 30, 230));
    let item_bg = color_from_hex(&config.colors.item, (50, 50, 50, 255));
    let sel_bg = color_from_hex(&config.colors.selected, (60, 120, 216, 255));
    let title_color = argb_from_hex(&config.colors.title, (255, 255, 255, 255));
    let appid_color = argb_from_hex(&config.colors.app_id, (170, 170, 170, 255));

    let padding = config.layout.padding as f32;
    let item_h = config.layout.item_height as f32;
    let item_sp = config.layout.item_spacing as f32;
    let corner_r = config.layout.corner_radius;
    let item_corner_r = config.layout.item_corner_radius;

    // Clear to transparent
    pixmap.fill(tiny_skia::Color::TRANSPARENT);

    // Draw background rounded rect
    draw_rounded_rect(
        &mut pixmap,
        0.0,
        0.0,
        width as f32,
        height as f32,
        corner_r,
        bg,
    );

    let item_w = width as f32 - padding * 2.0;

    // Calculate max text width for truncation
    let base_text_x_with_icon = ICON_LEFT_MARGIN + ICON_SIZE + ICON_TEXT_GAP;
    let base_text_x_no_icon = TEXT_LEFT_MARGIN;

    for (i, window) in windows.iter().enumerate() {
        let y = padding + (i as f32) * (item_h + item_sp);
        let x = padding;

        // Item background
        let color = if i == selected { sel_bg } else { item_bg };

        draw_rounded_rect(&mut pixmap, x, y, item_w, item_h, item_corner_r, color);

        // Determine text X offset based on whether we have an icon
        let icon = icons.get(i).and_then(|o| *o);
        let text_x = if icon.is_some() {
            x + base_text_x_with_icon as f32
        } else {
            x + base_text_x_no_icon as f32
        };

        // Available width for text (item width minus text_x offset minus right padding)
        let max_text_w = item_w - (text_x - x) - TEXT_LEFT_MARGIN as f32;

        // Draw icon if available
        if let Some(icon_data) = icon {
            let icon_y = y + (item_h - ICON_SIZE as f32) / 2.0;
            draw_icon(
                &mut pixmap,
                (x + ICON_LEFT_MARGIN as f32) as u32,
                icon_y as u32,
                ICON_SIZE,
                ICON_SIZE,
                icon_data,
            );
        }

        // Truncate title to fit within available width
        let title = truncate_to_width(&window.title, TITLE_FONT_SIZE, max_text_w, font);

        // Draw title text
        let data = pixmap.data_mut();
        font.draw_text(
            data,
            width,
            height,
            text_x as u32,
            (y + 6.0) as u32,
            &title,
            title_color,
            TITLE_FONT_SIZE,
        );

        // Truncate app_id to fit
        let app_id = truncate_to_width(&window.app_id, APPID_FONT_SIZE, max_text_w, font);

        // Draw app_id below title
        let data = pixmap.data_mut();
        font.draw_text(
            data,
            width,
            height,
            text_x as u32,
            (y + 26.0) as u32,
            &app_id,
            appid_color,
            APPID_FONT_SIZE,
        );
    }
}

/// Truncate a string so it fits within `max_width` pixels, appending "..." if needed.
fn truncate_to_width(text: &str, size_px: f32, max_width: f32, font: &mut FontRenderer) -> String {
    let full_width = font.measure_text(text, size_px);
    if full_width <= max_width {
        return text.to_string();
    }

    let ellipsis_width = font.measure_text("...", size_px);
    let target_width = max_width - ellipsis_width;
    if target_width <= 0.0 {
        return "...".to_string();
    }

    let mut width = 0.0f32;
    let mut end = 0;
    for (i, ch) in text.char_indices() {
        let glyph_w = font.measure_text(&text[i..i + ch.len_utf8()], size_px);
        if width + glyph_w > target_width {
            break;
        }
        width += glyph_w;
        end = i + ch.len_utf8();
    }

    format!("{}...", &text[..end])
}

fn draw_rounded_rect(pixmap: &mut PixmapMut, x: f32, y: f32, w: f32, h: f32, r: f32, color: Color) {
    let r = r.min(w / 2.0).min(h / 2.0);
    let mut pb = PathBuilder::new();

    // Top-left corner
    pb.move_to(x + r, y);
    // Top edge
    pb.line_to(x + w - r, y);
    // Top-right corner
    pb.quad_to(x + w, y, x + w, y + r);
    // Right edge
    pb.line_to(x + w, y + h - r);
    // Bottom-right corner
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    // Bottom edge
    pb.line_to(x + r, y + h);
    // Bottom-left corner
    pb.quad_to(x, y + h, x, y + h - r);
    // Left edge
    pb.line_to(x, y + r);
    // Top-left corner
    pb.quad_to(x, y, x + r, y);
    pb.close();

    let path = pb.finish().unwrap();
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;

    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

/// Draw an icon scaled to target_w x target_h at position (dest_x, dest_y).
/// Source is RGBA, destination is BGRA (little-endian ARGB8888).
/// Uses nearest-neighbor scaling and alpha blending.
fn draw_icon(
    pixmap: &mut PixmapMut,
    dest_x: u32,
    dest_y: u32,
    target_w: u32,
    target_h: u32,
    icon: &IconData,
) {
    let canvas_w = pixmap.width();
    let canvas_h = pixmap.height();
    let data = pixmap.data_mut();

    let src_w = icon.width;
    let src_h = icon.height;
    if src_w == 0 || src_h == 0 {
        return;
    }

    for ty in 0..target_h {
        let py = dest_y + ty;
        if py >= canvas_h {
            break;
        }
        // Nearest-neighbor: map target pixel to source pixel
        let sy = (ty as u64 * src_h as u64 / target_h as u64) as u32;
        let sy = sy.min(src_h - 1);

        for tx in 0..target_w {
            let px = dest_x + tx;
            if px >= canvas_w {
                break;
            }
            let sx = (tx as u64 * src_w as u64 / target_w as u64) as u32;
            let sx = sx.min(src_w - 1);

            let src_idx = ((sy * src_w + sx) * 4) as usize;
            if src_idx + 3 >= icon.pixels.len() {
                continue;
            }

            let sr = icon.pixels[src_idx] as u32;
            let sg = icon.pixels[src_idx + 1] as u32;
            let sb = icon.pixels[src_idx + 2] as u32;
            let sa = icon.pixels[src_idx + 3] as u32;

            if sa == 0 {
                continue;
            }

            let dst_idx = ((py * canvas_w + px) * 4) as usize;
            if dst_idx + 3 >= data.len() {
                continue;
            }

            if sa == 255 {
                // Fully opaque — direct write (BGRA order)
                data[dst_idx] = sb as u8;
                data[dst_idx + 1] = sg as u8;
                data[dst_idx + 2] = sr as u8;
                data[dst_idx + 3] = 255;
            } else {
                // Alpha blend: out = src * sa + dst * (255 - sa)
                // tiny-skia stores premultiplied alpha in BGRA order
                let db = data[dst_idx] as u32;
                let dg = data[dst_idx + 1] as u32;
                let dr = data[dst_idx + 2] as u32;
                let da = data[dst_idx + 3] as u32;

                let inv_sa = 255 - sa;
                let out_r = (sr * sa + dr * inv_sa + 128) / 255;
                let out_g = (sg * sa + dg * inv_sa + 128) / 255;
                let out_b = (sb * sa + db * inv_sa + 128) / 255;
                let out_a = (sa * 255 + da * inv_sa + 128) / 255;

                data[dst_idx] = out_b.min(255) as u8;
                data[dst_idx + 1] = out_g.min(255) as u8;
                data[dst_idx + 2] = out_r.min(255) as u8;
                data[dst_idx + 3] = out_a.min(255) as u8;
            }
        }
    }
}
