//! Procedurally drawn icons (no image files): toolbar pictograms, file icons, window icon.

use egui::{pos2, vec2, Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, StrokeKind};

pub const ACCENT: Color32 = Color32::from_rgb(0x1E, 0x6F, 0xD9);
pub const ACCENT_DARK: Color32 = Color32::from_rgb(0x14, 0x4E, 0x9E);
pub const BOX: Color32 = Color32::from_rgb(0xE8, 0x9B, 0x2E);
pub const BOX_DARK: Color32 = Color32::from_rgb(0xB8, 0x6E, 0x12);
pub const GREEN: Color32 = Color32::from_rgb(0x2E, 0x9E, 0x5B);
pub const RED: Color32 = Color32::from_rgb(0xD6, 0x45, 0x3D);
pub const FOLDER: Color32 = Color32::from_rgb(0xF3, 0xC1, 0x4B);
pub const FOLDER_DARK: Color32 = Color32::from_rgb(0xD0, 0x98, 0x1E);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Open,
    New,
    Extract,
    Add,
    Delete,
    View,
    Up,
    Info,
}

fn grey(c: Color32) -> Color32 {
    let l = ((c.r() as u32 * 3 + c.g() as u32 * 6 + c.b() as u32) / 10) as u8;
    let l = 160u8.saturating_add(l / 4);
    Color32::from_rgb(l, l, l)
}

fn arrow(p: &Painter, from: Pos2, to: Pos2, color: Color32, width: f32) {
    p.line_segment([from, to], Stroke::new(width, color));
    let dir = (to - from).normalized();
    let side = vec2(-dir.y, dir.x);
    let head = width * 2.2;
    p.add(Shape::convex_polygon(
        vec![to + dir * head * 0.6, to - dir * head + side * head, to - dir * head - side * head],
        color,
        Stroke::NONE,
    ));
}

/// The archive "box" with a zipper stripe, used in several icons.
pub fn archive_box(p: &Painter, r: Rect, enabled: bool) {
    let (body, dark) = if enabled { (BOX, BOX_DARK) } else { (grey(BOX), grey(BOX_DARK)) };
    p.rect(r, CornerRadius::same(3), body, Stroke::new(1.5, dark), StrokeKind::Inside);
    let lid = Rect::from_min_max(r.min, pos2(r.max.x, r.min.y + r.height() * 0.28));
    p.rect(lid, CornerRadius { nw: 3, ne: 3, sw: 0, se: 0 }, dark, Stroke::NONE, StrokeKind::Inside);
    // zipper teeth
    let cx = r.center().x;
    let mut y = lid.max.y + 2.0;
    let mut left = true;
    while y < r.max.y - 3.0 {
        let x0 = if left { cx - 3.0 } else { cx };
        p.rect_filled(Rect::from_min_size(pos2(x0, y), vec2(3.0, 2.0)), 0, Color32::WHITE);
        y += 3.0;
        left = !left;
    }
}

pub fn folder(p: &Painter, r: Rect) {
    let r = r.shrink(1.0);
    let tab = Rect::from_min_size(r.min + vec2(0.0, 1.0), vec2(r.width() * 0.45, r.height() * 0.3));
    p.rect_filled(tab, 2, FOLDER_DARK);
    let body = Rect::from_min_max(pos2(r.min.x, r.min.y + r.height() * 0.22), r.max);
    p.rect(body, 2, FOLDER, Stroke::new(1.0, FOLDER_DARK), StrokeKind::Inside);
}

/// A document icon coloured by file type.
pub fn file(p: &Painter, r: Rect, name: &str) {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    let color = match ext.as_str() {
        "zip" | "7z" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "rar" | "alz" | "egg" => BOX,
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "svg" => Color32::from_rgb(0x8E, 0x5C, 0xD9),
        "mp3" | "wav" | "flac" | "ogg" | "mp4" | "mkv" | "avi" | "mov" => Color32::from_rgb(0xD9, 0x5C, 0x9A),
        "exe" | "msi" | "dll" | "bat" | "sh" => RED,
        "txt" | "md" | "log" | "ini" | "csv" => Color32::from_rgb(0x7A, 0x86, 0x99),
        "doc" | "docx" | "hwp" | "hwpx" | "pdf" | "xls" | "xlsx" | "ppt" | "pptx" => ACCENT,
        "rs" | "c" | "cpp" | "h" | "py" | "js" | "ts" | "html" | "css" | "json" | "xml" => GREEN,
        _ => Color32::from_rgb(0x9A, 0xA4, 0xB2),
    };
    let w = r.height() * 0.78;
    let body = Rect::from_center_size(r.center(), vec2(w, r.height() - 2.0));
    let fold = w * 0.35;
    let pts = vec![
        body.left_top(),
        pos2(body.right() - fold, body.top()),
        pos2(body.right(), body.top() + fold),
        body.right_bottom(),
        body.left_bottom(),
    ];
    p.add(Shape::convex_polygon(pts, Color32::WHITE, Stroke::new(1.0, Color32::from_gray(140))));
    p.add(Shape::convex_polygon(
        vec![pos2(body.right() - fold, body.top()), pos2(body.right(), body.top() + fold), pos2(body.right() - fold, body.top() + fold)],
        Color32::from_gray(205),
        Stroke::new(1.0, Color32::from_gray(140)),
    ));
    let band = Rect::from_min_max(pos2(body.left() + 1.0, body.bottom() - body.height() * 0.42), pos2(body.right() - 1.0, body.bottom() - 1.0));
    p.rect_filled(band, 1, color);
}

pub fn lock(p: &Painter, r: Rect) {
    let c = Color32::from_rgb(0xC0, 0x8A, 0x10);
    let body = Rect::from_min_max(pos2(r.left() + 2.0, r.center().y - 1.0), pos2(r.right() - 2.0, r.bottom() - 1.0));
    p.circle_stroke(pos2(r.center().x, body.top()), body.width() * 0.32, Stroke::new(1.6, c));
    p.rect_filled(body, 2, c);
}

/// Toolbar pictogram, drawn inside `r` (about 30x30).
pub fn tool(p: &Painter, r: Rect, tool: Tool, enabled: bool) {
    let col = |c: Color32| if enabled { c } else { grey(c) };
    let c = r.center();
    match tool {
        Tool::Open => {
            let f = Rect::from_center_size(c + vec2(0.0, 2.0), vec2(28.0, 22.0));
            p.rect_filled(Rect::from_min_size(f.min - vec2(0.0, 3.0), vec2(12.0, 6.0)), 2, col(FOLDER_DARK));
            p.rect(f, 3, col(FOLDER), Stroke::new(1.5, col(FOLDER_DARK)), StrokeKind::Inside);
            archive_box(p, Rect::from_center_size(c + vec2(2.0, 4.0), vec2(14.0, 13.0)), enabled);
        }
        Tool::New => {
            archive_box(p, Rect::from_center_size(c + vec2(-3.0, 2.0), vec2(22.0, 22.0)), enabled);
            p.circle_filled(c + vec2(9.0, -8.0), 7.5, col(GREEN));
            p.line_segment([c + vec2(5.0, -8.0), c + vec2(13.0, -8.0)], Stroke::new(2.2, Color32::WHITE));
            p.line_segment([c + vec2(9.0, -12.0), c + vec2(9.0, -4.0)], Stroke::new(2.2, Color32::WHITE));
        }
        Tool::Extract => {
            archive_box(p, Rect::from_center_size(c + vec2(-4.0, 3.0), vec2(20.0, 20.0)), enabled);
            arrow(p, c + vec2(2.0, 2.0), c + vec2(12.0, -10.0), col(ACCENT), 3.0);
        }
        Tool::Add => {
            archive_box(p, Rect::from_center_size(c + vec2(-3.0, 3.0), vec2(20.0, 20.0)), enabled);
            arrow(p, c + vec2(13.0, -12.0), c + vec2(4.0, -1.0), col(GREEN), 3.0);
        }
        Tool::Delete => {
            let bin = Rect::from_center_size(c + vec2(0.0, 3.0), vec2(18.0, 20.0));
            p.rect(bin, CornerRadius { nw: 0, ne: 0, sw: 3, se: 3 }, col(Color32::from_gray(225)), Stroke::new(1.5, col(RED)), StrokeKind::Inside);
            p.line_segment([pos2(bin.left() - 3.0, bin.top() - 2.0), pos2(bin.right() + 3.0, bin.top() - 2.0)], Stroke::new(2.5, col(RED)));
            p.line_segment([pos2(c.x - 3.0, bin.top() - 5.0), pos2(c.x + 3.0, bin.top() - 5.0)], Stroke::new(2.5, col(RED)));
            for dx in [-4.5f32, 0.0, 4.5] {
                p.line_segment([pos2(c.x + dx, bin.top() + 4.0), pos2(c.x + dx, bin.bottom() - 3.0)], Stroke::new(1.5, col(RED)));
            }
        }
        Tool::View => {
            file(p, Rect::from_center_size(c + vec2(-4.0, 0.0), vec2(22.0, 26.0)), "a.txt");
            p.circle_stroke(c + vec2(5.0, 4.0), 6.0, Stroke::new(2.2, col(ACCENT_DARK)));
            p.line_segment([c + vec2(9.5, 8.5), c + vec2(14.0, 13.0)], Stroke::new(3.0, col(ACCENT_DARK)));
        }
        Tool::Up => {
            folder(p, Rect::from_center_size(c + vec2(0.0, 4.0), vec2(26.0, 20.0)));
            arrow(p, c + vec2(0.0, 10.0), c + vec2(0.0, -12.0), col(ACCENT), 3.0);
        }
        Tool::Info => {
            p.circle_filled(c, 13.0, col(ACCENT));
            p.circle_filled(c + vec2(0.0, -6.0), 2.2, Color32::WHITE);
            p.line_segment([c + vec2(0.0, -1.5), c + vec2(0.0, 8.0)], Stroke::new(3.2, Color32::WHITE));
        }
    }
}

/// RGBA pixels of the window icon (an orange box with a blue band and zipper).
pub fn app_icon_rgba(size: u32) -> Vec<u8> {
    let mut px = vec![0u8; (size * size * 4) as usize];
    let s = size as f32;
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let m = s * 0.08;
            let rad = s * 0.18;
            // rounded square mask
            let dx = (fx - s / 2.0).abs() - (s / 2.0 - m - rad);
            let dy = (fy - s / 2.0).abs() - (s / 2.0 - m - rad);
            let outside = dx.max(0.0).hypot(dy.max(0.0)) - rad;
            if outside > 0.5 {
                continue;
            }
            let alpha = (0.5 - outside).clamp(0.0, 1.0);
            let mut c = if fy < s * 0.36 { ACCENT } else { BOX };
            let cx = s / 2.0;
            if fy > s * 0.40 && fy < s * 0.86 {
                let row = ((fy - s * 0.40) / (s * 0.07)) as i32;
                let left = row % 2 == 0;
                let tooth = if left { fx > cx - s * 0.08 && fx < cx } else { fx >= cx && fx < cx + s * 0.08 };
                let gap = ((fy - s * 0.40) % (s * 0.07)) > s * 0.05;
                if tooth && !gap {
                    c = Color32::WHITE;
                }
            }
            if fy > s * 0.33 && fy < s * 0.39 {
                c = ACCENT_DARK;
            }
            let i = ((y * size + x) * 4) as usize;
            px[i] = c.r();
            px[i + 1] = c.g();
            px[i + 2] = c.b();
            px[i + 3] = (alpha * 255.0) as u8;
        }
    }
    px
}
