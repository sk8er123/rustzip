//! `--screenshot out.png`: draw about 120 frames, save the window as PNG, exit.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use crate::app::RustZipApp;

const FRAMES_BEFORE_SHOT: u64 = 120;

pub fn drive(app: &mut RustZipApp, ctx: &egui::Context) {
    let busy = app.busy();
    let frame = app.frame;
    let Some(shot) = &mut app.screenshot else { return };
    // Keep drawing even when the window is in the background or unfocused.
    ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
    ctx.request_repaint();
    if frame >= FRAMES_BEFORE_SHOT && !shot.requested && !busy {
        shot.requested = true;
        let slot = shot.image.clone();
        let ctx2 = ctx.clone();
        ctx.request_screenshot(move |img| {
            if let Ok(mut s) = slot.lock() {
                *s = Some(img);
            }
            ctx2.request_repaint();
        });
    }
    let img = shot.image.lock().ok().and_then(|mut s| s.take());
    if let Some(img) = img {
        if let Err(e) = save_png(&shot.path, &img) {
            eprintln!("{}", trf!("스크린샷 저장 실패: {e}", "Could not save the screenshot: {e}"));
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

pub fn save_png(path: &Path, img: &egui::ColorImage) -> anyhow::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let [w, h] = img.size;
    let mut enc = png::Encoder::new(BufWriter::new(File::create(path)?), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header()?;
    let mut data = Vec::with_capacity(w * h * 4);
    for p in &img.pixels {
        let [r, g, b, _] = p.to_array();
        data.extend_from_slice(&[r, g, b, 255]);
    }
    writer.write_image_data(&data)?;
    Ok(())
}
