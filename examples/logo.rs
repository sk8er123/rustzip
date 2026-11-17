//! Draws the RustZip logo: the app's archive box (lid + zipper) in front of a cog.
//!
//!     cargo run --example logo            # writes assets/logo.svg and assets/logo.png
//!
//! One list of shapes feeds both outputs, so the SVG and the PNG always match.

use std::f32::consts::TAU;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;

const SIZE: f32 = 512.0;

#[derive(Clone, Copy)]
struct Rgb(u8, u8, u8);

impl Rgb {
    fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.0, self.1, self.2)
    }
    fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
        let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
        Rgb(m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
    }
}

/// Same colours as `src/icons.rs`.
const BOX: Rgb = Rgb(0xE8, 0x9B, 0x2E);
const BOX_DARK: Rgb = Rgb(0xB8, 0x6E, 0x12);
const BOX_LIGHT: Rgb = Rgb(0xF5, 0xB8, 0x55);
const COG: Rgb = Rgb(0xC9, 0x4A, 0x24);
const COG_DARK: Rgb = Rgb(0x9A, 0x33, 0x16);
const TILE_TOP: Rgb = Rgb(0x2A, 0x33, 0x45);
const TILE_BOTTOM: Rgb = Rgb(0x15, 0x1A, 0x24);
const WHITE: Rgb = Rgb(0xFF, 0xFF, 0xFF);

#[derive(Clone, Copy)]
enum Fill {
    Solid(Rgb),
    /// Top-to-bottom gradient between two y positions.
    Vertical(Rgb, Rgb, f32, f32),
}

impl Fill {
    fn at(self, y: f32) -> Rgb {
        match self {
            Fill::Solid(c) => c,
            Fill::Vertical(a, b, y0, y1) => Rgb::mix(a, b, ((y - y0) / (y1 - y0)).clamp(0.0, 1.0)),
        }
    }
}

enum Shape {
    /// Rectangle with separate top and bottom corner radii.
    RoundRect { x0: f32, y0: f32, x1: f32, y1: f32, top: f32, bottom: f32 },
    Circle { cx: f32, cy: f32, r: f32 },
    /// Cog outline with a round hole.
    Cog { cx: f32, cy: f32, outer: f32, root: f32, hole: f32, teeth: u32 },
}

impl Shape {
    fn contains(&self, x: f32, y: f32) -> bool {
        match *self {
            Shape::RoundRect { x0, y0, x1, y1, top, bottom } => {
                if x < x0 || x > x1 || y < y0 || y > y1 {
                    return false;
                }
                let r = if y < (y0 + y1) / 2.0 { top } else { bottom };
                let cx = x.clamp(x0 + r, x1 - r);
                let cy = y.clamp(y0 + r, y1 - r);
                (x - cx).powi(2) + (y - cy).powi(2) <= r * r
            }
            Shape::Circle { cx, cy, r } => (x - cx).powi(2) + (y - cy).powi(2) <= r * r,
            Shape::Cog { cx, cy, hole, .. } => {
                let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
                d >= hole && point_in_polygon(&self.cog_points(), x, y)
            }
        }
    }

    fn cog_points(&self) -> Vec<(f32, f32)> {
        let Shape::Cog { cx, cy, outer, root, teeth, .. } = *self else { return Vec::new() };
        let step = TAU / teeth as f32;
        // Each tooth: rise, flat top, fall, flat root (fractions of one step).
        let profile = [(0.0, root), (0.12, outer), (0.38, outer), (0.5, root)];
        let mut pts = Vec::new();
        for i in 0..teeth {
            for (f, r) in profile {
                let a = (i as f32 + f) * step - TAU / 4.0;
                pts.push((cx + r * a.cos(), cy + r * a.sin()));
            }
        }
        pts
    }

    fn svg(&self, fill: &str) -> String {
        match *self {
            Shape::RoundRect { x0, y0, x1, y1, top, bottom } => {
                let (t, b) = (top, bottom);
                format!(
                    "<path d=\"M{} {y0}H{}A{t} {t} 0 0 1 {x1} {}V{}A{b} {b} 0 0 1 {} {y1}H{}A{b} {b} 0 0 1 {x0} {}V{}A{t} {t} 0 0 1 {} {y0}Z\" fill=\"{fill}\"/>",
                    x0 + t, x1 - t, y0 + t, y1 - b, x1 - b, x0 + b, y1 - b, y0 + t, x0 + t
                )
            }
            Shape::Circle { cx, cy, r } => format!("<circle cx=\"{cx}\" cy=\"{cy}\" r=\"{r}\" fill=\"{fill}\"/>"),
            Shape::Cog { cx, cy, hole, .. } => {
                let mut d = String::new();
                for (i, (x, y)) in self.cog_points().iter().enumerate() {
                    let _ = write!(d, "{}{x:.2} {y:.2}", if i == 0 { "M" } else { "L" });
                }
                let _ = write!(d, "ZM{} {cy}a{hole} {hole} 0 1 0 {} 0a{hole} {hole} 0 1 0 {} 0Z", cx - hole, 2.0 * hole, -2.0 * hole);
                format!("<path d=\"{d}\" fill=\"{fill}\" fill-rule=\"evenodd\"/>")
            }
        }
    }
}

fn point_in_polygon(pts: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut inside = false;
    let mut j = pts.len() - 1;
    for i in 0..pts.len() {
        let (xi, yi) = pts[i];
        let (xj, yj) = pts[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// The logo, back to front.
fn layers() -> Vec<(Shape, Fill)> {
    let c = SIZE / 2.0;
    let mut v = vec![
        (Shape::RoundRect { x0: 0.0, y0: 0.0, x1: SIZE, y1: SIZE, top: 112.0, bottom: 112.0 }, Fill::Vertical(TILE_TOP, TILE_BOTTOM, 0.0, SIZE)),
        (Shape::Cog { cx: c, cy: c, outer: 206.0, root: 178.0, hole: 140.0, teeth: 16 }, Fill::Vertical(COG, COG_DARK, 50.0, 462.0)),
        // box body, lid and a lighter edge under the lid
        (Shape::RoundRect { x0: 136.0, y0: 150.0, x1: 376.0, y1: 380.0, top: 22.0, bottom: 22.0 }, Fill::Vertical(BOX, BOX_DARK, 150.0, 420.0)),
        (Shape::RoundRect { x0: 136.0, y0: 150.0, x1: 376.0, y1: 218.0, top: 22.0, bottom: 0.0 }, Fill::Solid(BOX_DARK)),
        (Shape::RoundRect { x0: 136.0, y0: 218.0, x1: 376.0, y1: 228.0, top: 0.0, bottom: 0.0 }, Fill::Solid(BOX_LIGHT)),
    ];
    // zipper teeth, alternating left and right of the centre line
    let mut y = 240.0;
    let mut left = true;
    while y + 12.0 <= 364.0 {
        let x0 = if left { c - 22.0 } else { c };
        v.push((Shape::RoundRect { x0, y0: y, x1: x0 + 22.0, y1: y + 12.0, top: 3.0, bottom: 3.0 }, Fill::Solid(WHITE)));
        y += 18.0;
        left = !left;
    }
    // zipper pull: tab on the lid with a ring
    v.push((Shape::RoundRect { x0: c - 15.0, y0: 176.0, x1: c + 15.0, y1: 236.0, top: 8.0, bottom: 8.0 }, Fill::Solid(WHITE)));
    v.push((Shape::Circle { cx: c, cy: 192.0, r: 7.0 }, Fill::Solid(BOX_DARK)));
    v
}

fn svg(layers: &[(Shape, Fill)]) -> String {
    let mut defs = String::new();
    let mut body = String::new();
    for (i, (shape, fill)) in layers.iter().enumerate() {
        let paint = match *fill {
            Fill::Solid(c) => c.hex(),
            Fill::Vertical(a, b, y0, y1) => {
                let _ = write!(
                    defs,
                    "<linearGradient id=\"g{i}\" x1=\"0\" y1=\"{y0}\" x2=\"0\" y2=\"{y1}\" gradientUnits=\"userSpaceOnUse\"><stop offset=\"0\" stop-color=\"{}\"/><stop offset=\"1\" stop-color=\"{}\"/></linearGradient>",
                    a.hex(),
                    b.hex()
                );
                format!("url(#g{i})")
            }
        };
        body.push_str(&shape.svg(&paint));
        body.push('\n');
    }
    format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {SIZE} {SIZE}\" width=\"{SIZE}\" height=\"{SIZE}\">\n<title>RustZip</title>\n<defs>{defs}</defs>\n{body}</svg>\n")
}

/// Renders with 4×4 supersampling; transparent outside the tile.
fn raster(layers: &[(Shape, Fill)], px: u32) -> Vec<u8> {
    const SS: u32 = 4;
    let scale = SIZE / px as f32;
    let mut out = vec![0u8; (px * px * 4) as usize];
    for py in 0..px {
        for qx in 0..px {
            let (mut r, mut g, mut b, mut a) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (qx as f32 + (sx as f32 + 0.5) / SS as f32) * scale;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) * scale;
                    if let Some((_, fill)) = layers.iter().rev().find(|(s, _)| s.contains(x, y)) {
                        let c = fill.at(y);
                        r += c.0 as f32;
                        g += c.1 as f32;
                        b += c.2 as f32;
                        a += 1.0;
                    }
                }
            }
            let i = ((py * px + qx) * 4) as usize;
            if a > 0.0 {
                out[i] = (r / a).round() as u8;
                out[i + 1] = (g / a).round() as u8;
                out[i + 2] = (b / a).round() as u8;
                out[i + 3] = (a / (SS * SS) as f32 * 255.0).round() as u8;
            }
        }
    }
    out
}

fn save_png(path: &Path, rgba: &[u8], px: u32) -> Result<(), Box<dyn std::error::Error>> {
    let mut enc = png::Encoder::new(BufWriter::new(File::create(path)?), px, px);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgba)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    fs::create_dir_all(&dir)?;
    let layers = layers();
    fs::write(dir.join("logo.svg"), svg(&layers))?;
    for px in [512, 128] {
        let name = if px == 512 { "logo.png".to_string() } else { format!("logo-{px}.png") };
        save_png(&dir.join(&name), &raster(&layers, px), px)?;
    }
    println!("wrote {}", dir.display());
    Ok(())
}
