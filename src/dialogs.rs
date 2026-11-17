//! Pop-up windows: extract, create, add, delete, password, messages and progress.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use egui::{vec2, Align, Color32, Layout, RichText, Sense, Stroke};

use crate::app::{Action, AddDlg, CreateDlg, Dialog, ExtractDlg, JobKind, RustZipApp};
use crate::engine::{self, ArchiveKind, Method, Overwrite, PackOptions};
use crate::icons;
use crate::picker::{PickPurpose, PickResult, Picker};

fn buttons_row(ui: &mut egui::Ui, ok: &str, ok_width: f32) -> (bool, bool) {
    let mut res = (false, false);
    ui.horizontal(|ui| {
        res.0 = ui.add(egui::Button::new(RichText::new(ok).strong()).min_size(vec2(ok_width, 28.0))).clicked();
        res.1 = ui.add(egui::Button::new(tr!("취소", "Cancel")).min_size(vec2(70.0, 28.0))).clicked();
    });
    res
}

impl RustZipApp {
    pub fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut dlg) = self.dialog.take() else {
            self.show_picker(ctx);
            return;
        };
        let keep = match &mut dlg {
            Dialog::Extract(d) => self.extract_dialog(ctx, d),
            Dialog::Create(d) => self.create_dialog(ctx, d),
            Dialog::Add(d) => self.add_dialog(ctx, d),
            Dialog::ConfirmDelete(sel) => self.delete_dialog(ctx, sel),
            Dialog::Password { job, text, wrong } => self.password_dialog(ctx, job, text, *wrong),
            Dialog::Message { title, text, error } => message_dialog(ctx, title, text, *error),
            Dialog::DropChoice(paths) => self.drop_choice_dialog(ctx, paths),
            Dialog::About => message_dialog(
                ctx,
                tr!("RustZip 정보", "About RustZip"),
                tr!(
                    "RustZip 0.2\n\nRust와 egui로 만든 압축 프로그램입니다.\n\
                     열기: ZIP, 7Z, RAR, ALZ, EGG, LZH, ARJ, CAB, ISO, TAR, CPIO, AR(DEB),\n\
                     GZ, BZ2, XZ, LZMA, LZ, ZST, LZ4, BR, SZ, Z (TAR 안에 든 것 포함)\n\
                     만들기: ZIP, 7Z, TAR와 TAR.GZ 등, 단일 파일 GZ/BZ2/XZ/ZST/LZ4/LZMA/LZ/BR/SZ\n\
                     한글(CP949/EUC-KR) 파일 이름을 자동으로 알아봅니다.",
                    "RustZip 0.2\n\nAn archive manager written in Rust with egui.\n\
                     Opens: ZIP, 7Z, RAR, ALZ, EGG, LZH, ARJ, CAB, ISO, TAR, CPIO, AR(DEB),\n\
                     GZ, BZ2, XZ, LZMA, LZ, ZST, LZ4, BR, SZ, Z (also around TAR)\n\
                     Creates: ZIP, 7Z, TAR and TAR.GZ etc., single-file GZ/BZ2/XZ/ZST/LZ4/LZMA/LZ/BR/SZ\n\
                     Recognises Korean (CP949/EUC-KR) file names automatically."
                ),
                false,
            ),
        };
        // A chooser opened from the dialog is drawn on top of it.
        self.show_picker_for(ctx, &mut dlg);
        if keep && self.dialog.is_none() {
            self.dialog = Some(dlg);
        }
    }

    fn show_picker(&mut self, ctx: &egui::Context) {
        let Some(picker) = &mut self.picker else { return };
        match picker.show(ctx) {
            Some(PickResult::Paths(PickPurpose::OpenArchive, v)) => {
                self.picker = None;
                if let Some(p) = v.into_iter().next() {
                    self.actions.push(Action::OpenPath(p));
                }
            }
            Some(PickResult::Paths(PickPurpose::CreateSources, v)) => {
                self.picker = None;
                self.dialog = Some(Dialog::Create(Self::new_create_dialog(v)));
            }
            Some(PickResult::Paths(PickPurpose::AddSources, v)) => {
                self.picker = None;
                self.actions.push(Action::ShowAdd(v));
            }
            Some(_) => self.picker = None,
            None => {}
        }
    }

    fn show_picker_for(&mut self, ctx: &egui::Context, dlg: &mut Dialog) {
        let Some(picker) = &mut self.picker else { return };
        let Some(res) = picker.show(ctx) else { return };
        self.picker = None;
        let PickResult::Paths(purpose, v) = res else { return };
        match (dlg, purpose) {
            (Dialog::Extract(d), PickPurpose::ExtractDest) => {
                if let Some(p) = v.first() {
                    d.dest = p.display().to_string();
                }
            }
            (Dialog::Create(d), PickPurpose::CreateDest) => {
                if let Some(p) = v.first() {
                    d.dest = p.display().to_string();
                }
            }
            (Dialog::Create(CreateDlg { sources, .. }), PickPurpose::CreateSources) | (Dialog::Add(AddDlg { sources, .. }), PickPurpose::AddSources) => {
                for p in v {
                    if !sources.contains(&p) {
                        sources.push(p);
                    }
                }
            }
            _ => {}
        }
    }

    fn extract_dialog(&mut self, ctx: &egui::Context, d: &mut ExtractDlg) -> bool {
        let mut keep = true;
        let stem = self.archive_stem();
        let n_sel = self.selected.len();
        let m = egui::Modal::new(egui::Id::unique("dlg_extract")).show(ctx, |ui| {
            ui.set_width(520.0);
            ui.heading(tr!("압축 풀기", "Extract"));
            ui.add_space(6.0);
            ui.label(tr!("풀 위치:", "Extract to:"));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut d.dest).desired_width(400.0));
                if ui.button(tr!("찾아보기...", "Browse...")).clicked() {
                    self.picker = Some(Picker::new(PickPurpose::ExtractDest, Some(PathBuf::from(d.dest.trim())), String::new()));
                }
            });
            ui.checkbox(&mut d.own_folder, trf!("압축 파일 이름으로 새 폴더 만들기  ({stem})", "Create a folder named after the archive  ({stem})"));
            ui.add_space(6.0);
            ui.label(RichText::new(tr!("풀 파일", "Files to extract")).strong());
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.selected_only, false, tr!("모든 파일", "All files"));
                ui.add_enabled_ui(d.can_selected, |ui| {
                    ui.radio_value(&mut d.selected_only, true, trf!("선택한 파일 ({n_sel}개)", "Selected files ({n_sel})"));
                });
            });
            ui.add_space(4.0);
            ui.label(RichText::new(tr!("같은 이름의 파일이 있으면", "If a file with the same name exists")).strong());
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.overwrite, Overwrite::Rename, tr!("이름 바꿔 풀기", "Rename"));
                ui.radio_value(&mut d.overwrite, Overwrite::Always, tr!("덮어쓰기", "Overwrite"));
                ui.radio_value(&mut d.overwrite, Overwrite::Skip, tr!("건너뛰기", "Skip"));
            });
            ui.checkbox(&mut d.open_after, tr!("다 풀고 나서 폴더 열기", "Open the folder when done"));
            ui.add_space(8.0);
            let (ok, cancel) = buttons_row(ui, tr!("압축 풀기", "Extract"), 100.0);
            if ok {
                if let Some(job) = self.extract_job_from(d) {
                    self.last_extract_dir = Some(PathBuf::from(d.dest.trim()));
                    self.actions.push(Action::Start(job));
                }
                keep = false;
            }
            if cancel {
                keep = false;
            }
        });
        if m.should_close() && self.picker.is_none() {
            keep = false;
        }
        keep
    }

    fn level_slider(ui: &mut egui::Ui, level: &mut u8) {
        ui.horizontal(|ui| {
            ui.label(tr!("압축 수준:", "Compression level:"));
            ui.add(egui::Slider::new(level, 0..=9).show_value(true));
            let desc = match *level {
                0 => tr!("저장 (압축 안 함)", "Store (no compression)"),
                1..=2 => tr!("빠르게", "Fast"),
                3..=5 => tr!("조금 작게", "Smaller"),
                6..=7 => tr!("보통", "Normal"),
                _ => tr!("최대 압축", "Maximum"),
            };
            ui.weak(desc);
        });
    }

    /// Archive type and algorithm; changing the type also changes the file name ending.
    fn format_choice(ui: &mut egui::Ui, d: &mut CreateDlg) {
        let old = d.kind;
        ui.horizontal(|ui| {
            ui.label(tr!("압축 형식:", "Format:"));
            egui::ComboBox::from_id_salt("create_kind").selected_text(old.label()).height(400.0).show_ui(ui, |ui| {
                for k in ArchiveKind::creatable() {
                    let text = match k.filter {
                        engine::Filter::None => k.label(),
                        f => format!("{}  —  {}", k.label(), f.algorithm()),
                    };
                    ui.selectable_value(&mut d.kind, k, text);
                }
            });
        });
        if d.kind != old {
            d.dest = match d.sources.as_slice() {
                [only] if d.kind.single_file_only() && only.is_file() => format!("{}.{}", only.display(), d.kind.extension()),
                [only] if old.single_file_only() && only.is_file() => {
                    let stem = only.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    only.with_file_name(format!("{stem}.{}", d.kind.extension())).display().to_string()
                }
                _ => with_extension(&d.dest, old, d.kind),
            };
            if !d.kind.methods().contains(&d.method) {
                d.method = d.kind.methods().first().copied().unwrap_or(Method::Deflate);
            }
            d.error = None;
        }
        if d.kind.single_file_only() {
            ui.weak(tr!("이 형식은 파일 하나만 담습니다.", "This format holds a single file."));
        }
        let usable = d.kind.methods_for(d.use_password && d.kind.supports_password());
        if !usable.is_empty() && !usable.contains(&d.method) {
            d.method = usable[0];
        }
        Self::method_choice(ui, d.kind, &usable, &mut d.method);
    }

    fn method_choice(ui: &mut egui::Ui, kind: ArchiveKind, methods: &[Method], method: &mut Method) {
        if methods.is_empty() {
            return;
        }
        ui.horizontal(|ui| {
            ui.label(tr!("압축 방식:", "Method:"));
            egui::ComboBox::from_id_salt(("method", kind.label())).selected_text(method.label()).show_ui(ui, |ui| {
                for &m in methods {
                    ui.selectable_value(method, m, m.label());
                }
            });
        });
    }

    fn password_fields(ui: &mut egui::Ui, use_pw: &mut bool, pw: &mut String, pw2: Option<&mut String>, show: &mut bool) {
        ui.checkbox(use_pw, tr!("암호 걸기 (AES-256)", "Protect with a password (AES-256)"));
        if *use_pw {
            egui::Grid::new("pw_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                ui.label(tr!("암호:", "Password:"));
                ui.add(egui::TextEdit::singleline(pw).password(!*show).desired_width(260.0));
                ui.end_row();
                if let Some(pw2) = pw2 {
                    ui.label(tr!("암호 확인:", "Confirm password:"));
                    ui.add(egui::TextEdit::singleline(pw2).password(!*show).desired_width(260.0));
                    ui.end_row();
                }
            });
            ui.checkbox(show, tr!("암호 보이기", "Show password"));
        }
    }

    fn sources_list(ui: &mut egui::Ui, sources: &mut Vec<PathBuf>) {
        egui::Frame::new().stroke(Stroke::new(1.0, Color32::from_gray(200))).inner_margin(4).show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(140.0).min_scrolled_height(120.0).auto_shrink([false, false]).show(ui, |ui| {
                let mut remove = None;
                if sources.is_empty() {
                    ui.weak(tr!("(비어 있음 — 아래 버튼이나 끌어다 놓기로 추가하세요)", "(Empty — add items with the button below or by dragging them here)"));
                }
                for (i, s) in sources.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                        if s.is_dir() {
                            icons::folder(ui.painter(), r);
                        } else {
                            icons::file(ui.painter(), r, &s.to_string_lossy());
                        }
                        ui.add(egui::Label::new(s.display().to_string()).truncate());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button(tr!("빼기", "Remove")).clicked() {
                                remove = Some(i);
                            }
                        });
                    });
                }
                if let Some(i) = remove {
                    sources.remove(i);
                }
            });
        });
    }

    fn create_dialog(&mut self, ctx: &egui::Context, d: &mut CreateDlg) -> bool {
        let mut keep = true;
        let m = egui::Modal::new(egui::Id::unique("dlg_create")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.heading(tr!("새로 압축하기", "New archive"));
            ui.add_space(6.0);
            ui.label(RichText::new(tr!("압축할 파일과 폴더", "Files and folders to compress")).strong());
            Self::sources_list(ui, &mut d.sources);
            if ui.button(tr!("파일/폴더 추가...", "Add files/folders...")).clicked() {
                let start = d.sources.first().and_then(|p| p.parent().map(Path::to_path_buf));
                self.picker = Some(Picker::new(PickPurpose::CreateSources, start, String::new()));
            }
            ui.add_space(6.0);
            ui.label(RichText::new(tr!("저장할 압축 파일", "Save archive as")).strong());
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut d.dest).desired_width(430.0));
                if ui.button(tr!("찾아보기...", "Browse...")).clicked() {
                    let p = PathBuf::from(d.dest.trim());
                    let name = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| format!("{}.zip", tr!("새 압축 파일", "New archive")));
                    self.picker = Some(Picker::new(PickPurpose::CreateDest, p.parent().map(Path::to_path_buf), name));
                }
            });
            ui.add_space(6.0);
            Self::format_choice(ui, d);
            Self::level_slider(ui, &mut d.level);
            if d.kind.supports_password() {
                Self::password_fields(ui, &mut d.use_password, &mut d.password, Some(&mut d.password2), &mut d.show_password);
            } else {
                ui.weak(trf!("{} 형식은 암호를 걸 수 없습니다.", "{} archives cannot be password protected.", d.kind.label()));
            }
            if let Some(e) = &d.error {
                ui.colored_label(icons::RED, e);
            }
            ui.add_space(8.0);
            let (ok, cancel) = buttons_row(ui, tr!("압축 시작", "Compress"), 100.0);
            if ok {
                if let Some(job) = Self::create_job_from(d) {
                    self.actions.push(Action::Start(job));
                    keep = false;
                }
            }
            if cancel {
                keep = false;
            }
        });
        if m.should_close() && self.picker.is_none() {
            keep = false;
        }
        keep
    }

    fn add_dialog(&mut self, ctx: &egui::Context, d: &mut AddDlg) -> bool {
        let mut keep = true;
        let target = if self.cur_dir.is_empty() { tr!("(맨 위 폴더)", "(top folder)").to_string() } else { self.cur_dir.clone() };
        let m = egui::Modal::new(egui::Id::unique("dlg_add")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.heading(tr!("압축 파일에 추가", "Add to archive"));
            ui.label(trf!("넣을 위치: {target}", "Add into: {target}"));
            ui.add_space(6.0);
            Self::sources_list(ui, &mut d.sources);
            if ui.button(tr!("파일/폴더 추가...", "Add files/folders...")).clicked() {
                self.picker = Some(Picker::new(PickPurpose::AddSources, None, String::new()));
            }
            ui.add_space(6.0);
            Self::method_choice(ui, ArchiveKind::ZIP, ArchiveKind::ZIP.methods(), &mut d.method);
            Self::level_slider(ui, &mut d.level);
            Self::password_fields(ui, &mut d.use_password, &mut d.password, None, &mut d.show_password);
            ui.weak(tr!("같은 이름의 항목은 새 파일로 바뀝니다.", "Items with the same name are replaced by the new files."));
            ui.add_space(8.0);
            let (ok, cancel) = buttons_row(ui, tr!("추가", "Add"), 90.0);
            if ok && !d.sources.is_empty() {
                if let Some(l) = self.listing.clone() {
                    self.actions.push(Action::Start(JobKind::Add {
                        listing: l,
                        sources: d.sources.clone(),
                        base: self.cur_dir.clone(),
                        opts: PackOptions { method: d.method, level: d.level, password: (d.use_password && !d.password.is_empty()).then(|| d.password.clone()) },
                    }));
                }
                keep = false;
            }
            if cancel {
                keep = false;
            }
        });
        if m.should_close() && self.picker.is_none() {
            keep = false;
        }
        keep
    }

    fn delete_dialog(&mut self, ctx: &egui::Context, sel: &mut Vec<String>) -> bool {
        let mut keep = true;
        let m = egui::Modal::new(egui::Id::unique("dlg_delete")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(tr!("삭제 확인", "Confirm delete"));
            ui.add_space(4.0);
            let first = sel.first().map(|s| s.rsplit('/').next().unwrap_or(s).to_string()).unwrap_or_default();
            if sel.len() == 1 {
                ui.label(trf!("'{first}'을(를) 압축 파일에서 삭제할까요?", "Delete '{first}' from the archive?"));
            } else {
                let others = sel.len() - 1;
                ui.label(trf!("'{first}' 외 {others}개 항목을 압축 파일에서 삭제할까요?", "Delete '{first}' and {others} other item(s) from the archive?"));
            }
            ui.weak(tr!("삭제한 항목은 되돌릴 수 없습니다.", "This cannot be undone."));
            ui.add_space(8.0);
            let (ok, cancel) = buttons_row(ui, tr!("삭제", "Delete"), 80.0);
            if ok {
                if let Some(l) = self.listing.clone() {
                    self.actions.push(Action::Start(JobKind::Delete { listing: l, selection: sel.clone() }));
                }
                keep = false;
            }
            if cancel {
                keep = false;
            }
        });
        if m.should_close() {
            keep = false;
        }
        keep
    }

    fn password_dialog(&mut self, ctx: &egui::Context, job: &mut JobKind, text: &mut String, wrong: bool) -> bool {
        let mut keep = true;
        let m = egui::Modal::new(egui::Id::unique("dlg_password")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(tr!("암호 입력", "Enter password"));
            ui.add_space(4.0);
            if wrong {
                ui.colored_label(icons::RED, tr!("암호가 올바르지 않습니다. 다시 입력하세요.", "The password is incorrect. Please try again."));
            } else {
                ui.label(tr!("이 압축 파일은 암호로 보호되어 있습니다.", "This archive is password protected."));
            }
            let r = ui.add(egui::TextEdit::singleline(text).password(true).desired_width(300.0).hint_text(tr!("암호", "Password")));
            if text.is_empty() && !r.has_focus() {
                r.request_focus();
            }
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(8.0);
            let (ok, cancel) = buttons_row(ui, tr!("확인", "OK"), 80.0);
            if ok || enter {
                let mut j = job.clone();
                j.set_password(text.clone());
                self.actions.push(Action::Start(j));
                keep = false;
            }
            if cancel {
                keep = false;
            }
        });
        if m.should_close() {
            keep = false;
        }
        keep
    }

    fn drop_choice_dialog(&mut self, ctx: &egui::Context, paths: &mut Vec<PathBuf>) -> bool {
        let mut keep = true;
        let m = egui::Modal::new(egui::Id::unique("dlg_drop")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(tr!("끌어다 놓은 파일", "Dropped files"));
            let n = paths.len();
            ui.label(trf!("항목 {n}개를 어떻게 할까요?", "What should be done with {n} item(s)?"));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(tr!("지금 연 ZIP에 추가", "Add to the open ZIP")).clicked() {
                    self.actions.push(Action::ShowAdd(paths.clone()));
                    keep = false;
                }
                if ui.button(tr!("새로 압축하기", "New archive")).clicked() {
                    self.actions.push(Action::NewArchive(paths.clone()));
                    keep = false;
                }
                if ui.button(tr!("취소", "Cancel")).clicked() {
                    keep = false;
                }
            });
        });
        if m.should_close() {
            keep = false;
        }
        keep
    }

    pub fn progress_modal(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else { return };
        // Short jobs finish before the window would flash on screen.
        if job.started.elapsed().as_millis() < 250 {
            return;
        }
        let progress = job.progress.clone();
        let title = job.kind.title();
        egui::Modal::new(egui::Id::unique("dlg_progress")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.heading(title);
            ui.add_space(4.0);
            let cur = progress.current();
            ui.add(egui::Label::new(if cur.is_empty() { tr!("준비 중...", "Preparing...").to_string() } else { cur }).truncate());
            ui.add(egui::ProgressBar::new(progress.fraction()).show_percentage().animate(true));
            let done = progress.done.load(Ordering::Relaxed);
            let total = progress.total.load(Ordering::Relaxed);
            ui.weak(format!("{} / {}", engine::format_size(done), engine::format_size(total)));
            ui.add_space(6.0);
            if ui.add_enabled(!progress.cancelled(), egui::Button::new(tr!("취소", "Cancel")).min_size(vec2(70.0, 26.0))).clicked() {
                progress.cancel.store(true, Ordering::Relaxed);
            }
        });
    }
}

/// Swaps the archive-type ending of a file name (`자료.zip` → `자료.tar.zst`).
fn with_extension(dest: &str, old: ArchiveKind, new: ArchiveKind) -> String {
    let trimmed = dest.trim_end();
    let old_ext = format!(".{}", old.extension());
    let stem = if trimmed.to_lowercase().ends_with(&old_ext.to_lowercase()) { &trimmed[..trimmed.len() - old_ext.len()] } else { trimmed };
    format!("{stem}.{}", new.extension())
}

fn message_dialog(ctx: &egui::Context, title: &str, text: &str, error: bool) -> bool {
    let mut keep = true;
    let m = egui::Modal::new(egui::Id::unique("dlg_message")).show(ctx, |ui| {
        ui.set_max_width(480.0);
        ui.heading(RichText::new(title).color(if error { icons::RED } else { Color32::from_gray(20) }));
        ui.add_space(6.0);
        ui.label(text);
        ui.add_space(8.0);
        if ui.add(egui::Button::new(tr!("확인", "OK")).min_size(vec2(80.0, 28.0))).clicked() {
            keep = false;
        }
    });
    if m.should_close() {
        keep = false;
    }
    keep
}
