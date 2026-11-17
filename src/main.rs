//! RustZip — an archive manager with a Korean interface (egui/eframe, pure Rust).
//!
//! Usage:
//!   rustzip [archive]                 open an archive passed on the command line
//!   rustzip --screenshot out.png      draw ~120 frames, save a PNG, exit
//!   rustzip --self-test out.json      run the scripted self-test, save results, exit
//!   rustzip --lang ko|en              interface language for this run (default: saved choice, then the system's)

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[macro_use]
mod i18n;

mod app;
mod dialogs;
mod engine;
mod icons;
mod picker;
mod selftest;
mod shot;
mod views;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use app::{RustZipApp, ScreenshotState};

struct Args {
    archive: Option<PathBuf>,
    screenshot: Option<PathBuf>,
    selftest: Option<PathBuf>,
    lang: Option<i18n::Lang>,
}

fn parse_args() -> Args {
    let mut args = Args { archive: None, screenshot: None, selftest: None, lang: None };
    let mut it = std::env::args_os().skip(1);
    while let Some(a) = it.next() {
        match a.to_str() {
            Some("--screenshot") => args.screenshot = it.next().map(PathBuf::from),
            Some("--self-test") => args.selftest = it.next().map(PathBuf::from),
            Some("--lang") => {
                let code = it.next().map(|v| v.to_string_lossy().into_owned()).unwrap_or_default();
                args.lang = i18n::Lang::from_code(&code);
                if args.lang.is_none() {
                    eprintln!("--lang: unknown language '{code}' (use ko or en)");
                }
            }
            _ => {
                if args.archive.is_none() {
                    args.archive = Some(PathBuf::from(a));
                }
            }
        }
    }
    args
}

/// Finds a font with Hangul glyphs on this computer (Windows, macOS, Linux).
fn korean_font() -> Option<(Vec<u8>, u32)> {
    let candidates: &[(&str, u32)] = &[
        ("C:\\Windows\\Fonts\\malgun.ttf", 0),
        ("C:\\Windows\\Fonts\\gulim.ttc", 0),
        ("/System/Library/Fonts/AppleSDGothicNeo.ttc", 0),
        ("/System/Library/Fonts/Supplemental/AppleGothic.ttf", 0),
        ("/Library/Fonts/AppleGothic.ttf", 0),
        ("/usr/share/fonts/truetype/nanum/NanumGothic.ttf", 0),
        ("/usr/share/fonts/nanum/NanumGothic.ttf", 0),
        ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 1),
        ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 1),
        ("/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc", 1),
        ("/usr/share/fonts/opentype/noto/NotoSansKR-Regular.otf", 0),
        ("/usr/share/fonts/truetype/unfonts-core/UnDotum.ttf", 0),
    ];
    for (p, idx) in candidates {
        if let Ok(bytes) = std::fs::read(p) {
            return Some((bytes, *idx));
        }
    }
    // Last resort: look for any CJK/Nanum font file under the usual folders.
    for root in ["/usr/share/fonts", "/usr/local/share/fonts"] {
        if let Some(found) = find_font(Path::new(root), 0) {
            return std::fs::read(found).ok().map(|b| (b, 0));
        }
    }
    None
}

fn find_font(dir: &Path, depth: u32) -> Option<PathBuf> {
    if depth > 4 {
        return None;
    }
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(f) = find_font(&p, depth + 1) {
                return Some(f);
            }
        } else {
            let n = p.file_name()?.to_string_lossy().to_lowercase();
            if (n.contains("nanum") || n.contains("cjk") || n.contains("kr")) && (n.ends_with(".ttf") || n.ends_with(".otf") || n.ends_with(".ttc")) {
                return Some(p);
            }
        }
    }
    None
}

fn setup_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    if let Some((bytes, index)) = korean_font() {
        let mut fd = egui::FontData::from_owned(bytes);
        fd.index = index;
        fonts.font_data.insert("korean".into(), Arc::new(fd));
        for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(fam).or_default().insert(0, "korean".into());
        }
    } else {
        eprintln!("{}", tr!("한글 글꼴을 찾지 못했습니다. 한글이 네모로 보일 수 있습니다.", "No font with Korean characters was found; Korean text may show as boxes."));
    }
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::Theme::Light);
    ctx.all_styles_mut(|style| {
        style.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
        style.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
        style.text_styles.insert(egui::TextStyle::Heading, egui::FontId::proportional(19.0));
        style.visuals.selection.bg_fill = egui::Color32::from_rgb(0xC6, 0xDD, 0xFA);
        style.visuals.selection.stroke = egui::Stroke::new(1.0, icons::ACCENT_DARK);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
    });
}

/// Pseudo-random bytes (xorshift) that barely compress, like photos or music.
fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

/// Makes a small archive with Korean names to show in screenshots.
fn demo_archive(dir: &Path) -> anyhow::Result<PathBuf> {
    let src = dir.join("샘플 자료");
    let files: Vec<(&str, Vec<u8>)> = vec![
        ("문서/회의록 2026-11.txt", "회의 안건: 압축 프로그램 일정\n".repeat(400).into_bytes()),
        ("문서/예산안.csv", (0..3000).map(|i| format!("{i},항목{i},{}\n", i * 1300)).collect::<String>().into_bytes()),
        ("문서/제안서.pdf", noise(60_000, 2_654_435_761)),
        ("사진/여름 휴가/바다.jpg", noise(180_000, 1_103_515_245)),
        ("사진/가족.png", noise(90_000, 22_695_477)),
        ("음악/노래 모음.mp3", noise(240_000, 134_775_813)),
        ("읽어보세요.txt", "RustZip으로 만든 샘플 압축 파일입니다.\n".repeat(80).into_bytes()),
        ("설치 안내.html", "<p>설치 방법</p>\n".repeat(500).into_bytes()),
        ("main.rs", "fn main() { println!(\"안녕하세요\"); }\n".repeat(120).into_bytes()),
    ];
    for (rel, data) in files {
        let p = src.join(rel);
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(p, data)?;
    }
    let mut sources: Vec<PathBuf> = std::fs::read_dir(&src)?.flatten().map(|e| e.path()).collect();
    sources.sort();
    let dest = dir.join("샘플 자료.zip");
    engine::create(&dest, &sources, engine::ArchiveKind::ZIP, &engine::PackOptions::default(), &engine::Progress::default())?;
    Ok(dest)
}

fn main() -> eframe::Result<()> {
    let args = parse_args();
    i18n::set_lang(args.lang.or_else(i18n::saved_lang).unwrap_or_else(i18n::system_lang));
    let background = args.screenshot.is_some() || args.selftest.is_some();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("RustZip")
        .with_app_id("rustzip")
        .with_inner_size([1100.0, 700.0])
        .with_min_inner_size([720.0, 420.0])
        .with_drag_and_drop(true)
        .with_icon(egui::IconData { rgba: icons::app_icon_rgba(64), width: 64, height: 64 });
    if background {
        // Never steal focus from what the person is doing.
        viewport = viewport.with_active(false);
        #[cfg(windows)]
        {
            viewport = viewport.with_window_level(egui::WindowLevel::AlwaysOnBottom);
        }
    }
    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions { viewport, ..Default::default() };
    #[cfg(target_os = "macos")]
    if background {
        options.event_loop_builder = Some(Box::new(|builder| {
            use winit::platform::macos::EventLoopBuilderExtMacOS;
            builder.with_activate_ignoring_other_apps(false);
        }));
    }

    // Kept alive until the window closes.
    let demo_dir = tempfile::tempdir().ok();

    eframe::run_native(
        "RustZip",
        options,
        Box::new(move |cc| {
            setup_style(&cc.egui_ctx);
            let mut app = RustZipApp::new(args.archive.clone());
            if let Some(path) = &args.screenshot {
                app.quiet = true;
                if args.archive.is_none() {
                    if let Some(d) = &demo_dir {
                        if let Ok(zip) = demo_archive(d.path()) {
                            let _ = app.open_now(&zip);
                            app.navigate(String::new());
                            app.selected.insert("문서".into());
                        }
                    }
                }
                app.screenshot = Some(ScreenshotState { path: path.clone(), requested: false, image: Arc::new(Mutex::new(None)) });
            }
            if let Some(out) = &args.selftest {
                app.quiet = true;
                match selftest::SelfTest::new(out.clone()) {
                    Ok(st) => app.selftest = Some(st),
                    Err(e) => eprintln!("self-test setup failed: {e}"),
                }
            }
            Ok(Box::new(app))
        }),
    )
}
