//! `--self-test out.json`: drives the app like a person would (make a ZIP from
//! files, open folders inside it, extract everything) and records what worked.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::app::{Action, RustZipApp};
use crate::engine::ExtractReport;

pub struct SelfTest {
    pub out: PathBuf,
    dir: tempfile::TempDir,
    stage: u32,
    wait_until: u64,
    steps: Vec<serde_json::Value>,
    pub extracted: Option<ExtractReport>,
    pub errors: Vec<String>,
    done: bool,
}

/// Files created for the test (relative path, contents).
fn sample_files() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("샘플/읽어보세요.txt", "RustZip 자체 시험 파일입니다.\n".repeat(50).into_bytes()),
        ("샘플/문서/보고서.txt", "한글 보고서 내용 — 압축 시험\n".repeat(300).into_bytes()),
        ("샘플/문서/예산.csv", (0..2000).map(|i| format!("{i},{}\n", i * 7)).collect::<String>().into_bytes()),
        ("샘플/사진/바다.bin", (0..40_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect()),
    ]
}

impl SelfTest {
    pub fn new(out: PathBuf) -> std::io::Result<Self> {
        Ok(Self {
            out,
            dir: tempfile::tempdir()?,
            stage: 0,
            wait_until: 0,
            steps: Vec::new(),
            extracted: None,
            errors: Vec::new(),
            done: false,
        })
    }

    fn step(&mut self, name: &str, passed: bool, detail: String) {
        self.steps.push(json!({"step": name, "passed": passed, "detail": detail}));
    }

    fn write(&self) {
        let v = json!({"version": 1, "steps": self.steps});
        if let Some(parent) = self.out.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&self.out, serde_json::to_string_pretty(&v).unwrap_or_default());
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }
}

pub fn drive(app: &mut RustZipApp, ctx: &egui::Context) {
    let Some(mut st) = app.selftest.take() else { return };
    ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
    ctx.request_repaint();
    if !st.done {
        run_stage(app, ctx, &mut st);
        if app.frame > 3000 && !st.done {
            st.step("timeout", false, format!("stopped at stage {} after {} frames; errors: {:?}", st.stage, app.frame, st.errors));
            finish(app, ctx, &mut st);
        }
    }
    app.selftest = Some(st);
}

fn finish(app: &mut RustZipApp, ctx: &egui::Context, st: &mut SelfTest) {
    // Make sure all three core steps are present.
    for name in ["move", "act", "hud"] {
        if !st.steps.iter().any(|s| s["step"] == name) {
            st.step(name, false, format!("not reached (errors: {:?}, status: {})", st.errors, app.status));
        }
    }
    st.write();
    st.done = true;
    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
}

fn run_stage(app: &mut RustZipApp, ctx: &egui::Context, st: &mut SelfTest) {
    if app.frame < st.wait_until {
        return;
    }
    match st.stage {
        // 1. Put sample files on disk and press "압축 시작" in the new-archive dialog.
        0 => {
            if app.frame < 5 {
                return;
            }
            let root = st.root().to_path_buf();
            for (rel, data) in sample_files() {
                let p = root.join("src").join(rel);
                fs::create_dir_all(p.parent().unwrap()).ok();
                fs::write(&p, data).ok();
            }
            let mut dlg = RustZipApp::new_create_dialog(vec![root.join("src/샘플")]);
            dlg.dest = root.join("시험.zip").display().to_string();
            dlg.level = 6;
            match RustZipApp::create_job_from(&mut dlg) {
                Some(job) => app.apply(ctx, Action::Start(job)),
                None => st.errors.push(format!("create dialog refused: {:?}", dlg.error)),
            }
            st.stage = 1;
        }
        // 2. When the new archive is open, double-click into folders.
        1 => {
            if app.busy() {
                return;
            }
            let files = app.listing.as_ref().map(|l| l.file_count()).unwrap_or(0);
            if files != sample_files().len() {
                st.step("act", false, format!("created archive has {files} files, expected {}; errors {:?}", sample_files().len(), st.errors));
                finish(app, ctx, st);
                return;
            }
            let root_view: Vec<String> = app.view.iter().map(|c| c.name.clone()).collect();
            st.errors.retain(|_| true);
            app.apply(ctx, Action::Navigate("샘플".into()));
            app.apply(ctx, Action::Navigate("샘플/문서".into()));
            st.stage = 2;
            st.wait_until = app.frame + 3;
            st.steps.push(json!({"step": "_root", "passed": true, "detail": format!("{root_view:?}")}));
        }
        2 => {
            let names: Vec<String> = app.view.iter().map(|c| c.name.clone()).collect();
            let inside = app.cur_dir == "샘플/문서" && names.iter().any(|n| n == "보고서.txt") && app.hud.rows == names.len();
            app.apply(ctx, Action::Up);
            let up_ok = app.cur_dir == "샘플" && app.view.iter().any(|c| c.name == "문서" && c.is_dir);
            st.steps.retain(|s| s["step"] != "_root");
            st.step(
                "move",
                inside && up_ok,
                format!("opened folder '샘플/문서' (rows drawn: {}, items {:?}), then went up to '{}'", app.hud.rows, names, app.cur_dir),
            );
            // 3. Extract everything through the extract dialog.
            let mut dlg = app.new_extract_dialog();
            dlg.dest = st.root().join("풀기").display().to_string();
            dlg.own_folder = true;
            dlg.selected_only = false;
            match app.extract_job_from(&dlg) {
                Some(job) => app.apply(ctx, Action::Start(job)),
                None => st.errors.push("extract dialog gave no job".into()),
            }
            st.stage = 3;
        }
        3 => {
            if app.busy() {
                return;
            }
            let out = st.root().join("풀기").join("시험");
            let mut same = 0;
            let mut missing = Vec::new();
            for (rel, data) in sample_files() {
                match fs::read(out.join(rel)) {
                    Ok(got) if got == data => same += 1,
                    _ => missing.push(rel),
                }
            }
            let files = st.extracted.as_ref().map(|r| r.files).unwrap_or(0);
            let packed = app.listing.as_ref().map(|l| l.archive_size).unwrap_or(0);
            let total = app.listing.as_ref().map(|l| l.total_size()).unwrap_or(0);
            st.step(
                "act",
                missing.is_empty() && files == sample_files().len(),
                format!(
                    "compressed {} files ({} B -> {} B ZIP), extracted {files}, {same} identical byte-for-byte; missing/different: {missing:?}; errors: {:?}",
                    sample_files().len(),
                    total,
                    packed,
                    st.errors
                ),
            );
            st.stage = 4;
            st.wait_until = app.frame + 2;
        }
        4 => {
            let h = &app.hud;
            let want = [tr!("이름", "Name"), tr!("크기", "Size"), tr!("압축 크기", "Packed"), tr!("수정한 날짜", "Modified")];
            let cols_ok = want.iter().all(|w| h.columns.contains(w));
            let ok = cols_ok && h.toolbar && h.status && h.tree && h.path_bar && h.rows > 0;
            st.step(
                "hud",
                ok,
                format!(
                    "columns {:?}, rows {}, toolbar {}, folder tree {}, location bar {}, status bar {} ('{}')",
                    h.columns, h.rows, h.toolbar, h.tree, h.path_bar, h.status, app.status
                ),
            );
            finish(app, ctx, st);
        }
        _ => {}
    }
}
