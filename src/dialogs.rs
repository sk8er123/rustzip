//! Pop-up windows: extract, create, add, delete, password, messages and progress.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use egui::{vec2, Align, Color32, Layout, RichText, Sense, Stroke};

use crate::app::{Action, AddDlg, CreateDlg, Dialog, ExtractDlg, JobKind, RustZipApp};
use crate::engine::{self, Overwrite, ZipOptions};
use crate::icons;
use crate::picker::{PickPurpose, PickResult, Picker};

fn buttons_row(ui: &mut egui::Ui, ok: &str, ok_width: f32) -> (bool, bool) {
    let mut res = (false, false);
    ui.horizontal(|ui| {
        res.0 = ui.add(egui::Button::new(RichText::new(ok).strong()).min_size(vec2(ok_width, 28.0))).clicked();
        res.1 = ui.add(egui::Button::new("취소").min_size(vec2(70.0, 28.0))).clicked();
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
                "RustZip 정보",
                "RustZip 0.1\n\nRust와 egui로 만든 압축 프로그램입니다.\n\
                 열기: ZIP, 7Z, TAR, TAR.GZ, TAR.BZ2, TAR.XZ, GZ, BZ2, XZ\n\
                 만들기: ZIP (압축 수준 0~9, AES-256 암호)\n\
                 한글(CP949/EUC-KR) 파일 이름을 자동으로 알아봅니다.",
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
            ui.heading("압축 풀기");
            ui.add_space(6.0);
            ui.label("풀 위치:");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut d.dest).desired_width(400.0));
                if ui.button("찾아보기...").clicked() {
                    self.picker = Some(Picker::new(PickPurpose::ExtractDest, Some(PathBuf::from(d.dest.trim())), String::new()));
                }
            });
            ui.checkbox(&mut d.own_folder, format!("압축 파일 이름으로 새 폴더 만들기  ({stem})"));
            ui.add_space(6.0);
            ui.label(RichText::new("풀 파일").strong());
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.selected_only, false, "모든 파일");
                ui.add_enabled_ui(d.can_selected, |ui| {
                    ui.radio_value(&mut d.selected_only, true, format!("선택한 파일 ({n_sel}개)"));
                });
            });
            ui.add_space(4.0);
            ui.label(RichText::new("같은 이름의 파일이 있으면").strong());
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.overwrite, Overwrite::Rename, "이름 바꿔 풀기");
                ui.radio_value(&mut d.overwrite, Overwrite::Always, "덮어쓰기");
                ui.radio_value(&mut d.overwrite, Overwrite::Skip, "건너뛰기");
            });
            ui.checkbox(&mut d.open_after, "다 풀고 나서 폴더 열기");
            ui.add_space(8.0);
            let (ok, cancel) = buttons_row(ui, "압축 풀기", 100.0);
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
            ui.label("압축 수준:");
            ui.add(egui::Slider::new(level, 0..=9).show_value(true));
            let desc = match *level {
                0 => "저장 (압축 안 함)",
                1..=2 => "빠르게",
                3..=5 => "조금 작게",
                6..=7 => "보통",
                _ => "최대 압축",
            };
            ui.weak(desc);
        });
    }

    fn password_fields(ui: &mut egui::Ui, use_pw: &mut bool, pw: &mut String, pw2: Option<&mut String>, show: &mut bool) {
        ui.checkbox(use_pw, "암호 걸기 (AES-256)");
        if *use_pw {
            egui::Grid::new("pw_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                ui.label("암호:");
                ui.add(egui::TextEdit::singleline(pw).password(!*show).desired_width(260.0));
                ui.end_row();
                if let Some(pw2) = pw2 {
                    ui.label("암호 확인:");
                    ui.add(egui::TextEdit::singleline(pw2).password(!*show).desired_width(260.0));
                    ui.end_row();
                }
            });
            ui.checkbox(show, "암호 보이기");
        }
    }

    fn sources_list(ui: &mut egui::Ui, sources: &mut Vec<PathBuf>) {
        egui::Frame::new().stroke(Stroke::new(1.0, Color32::from_gray(200))).inner_margin(4).show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(140.0).min_scrolled_height(120.0).auto_shrink([false, false]).show(ui, |ui| {
                let mut remove = None;
                if sources.is_empty() {
                    ui.weak("(비어 있음 — 아래 버튼이나 끌어다 놓기로 추가하세요)");
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
                            if ui.small_button("빼기").clicked() {
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
            ui.heading("새로 압축하기 (ZIP)");
            ui.add_space(6.0);
            ui.label(RichText::new("압축할 파일과 폴더").strong());
            Self::sources_list(ui, &mut d.sources);
            if ui.button("파일/폴더 추가...").clicked() {
                let start = d.sources.first().and_then(|p| p.parent().map(Path::to_path_buf));
                self.picker = Some(Picker::new(PickPurpose::CreateSources, start, String::new()));
            }
            ui.add_space(6.0);
            ui.label(RichText::new("저장할 압축 파일").strong());
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut d.dest).desired_width(430.0));
                if ui.button("찾아보기...").clicked() {
                    let p = PathBuf::from(d.dest.trim());
                    let name = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "새 압축 파일.zip".into());
                    self.picker = Some(Picker::new(PickPurpose::CreateDest, p.parent().map(Path::to_path_buf), name));
                }
            });
            ui.add_space(6.0);
            Self::level_slider(ui, &mut d.level);
            Self::password_fields(ui, &mut d.use_password, &mut d.password, Some(&mut d.password2), &mut d.show_password);
            if let Some(e) = &d.error {
                ui.colored_label(icons::RED, e);
            }
            ui.add_space(8.0);
            let (ok, cancel) = buttons_row(ui, "압축 시작", 100.0);
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
        let target = if self.cur_dir.is_empty() { "(맨 위 폴더)".to_string() } else { self.cur_dir.clone() };
        let m = egui::Modal::new(egui::Id::unique("dlg_add")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.heading("압축 파일에 추가");
            ui.label(format!("넣을 위치: {target}"));
            ui.add_space(6.0);
            Self::sources_list(ui, &mut d.sources);
            if ui.button("파일/폴더 추가...").clicked() {
                self.picker = Some(Picker::new(PickPurpose::AddSources, None, String::new()));
            }
            ui.add_space(6.0);
            Self::level_slider(ui, &mut d.level);
            Self::password_fields(ui, &mut d.use_password, &mut d.password, None, &mut d.show_password);
            ui.weak("같은 이름의 항목은 새 파일로 바뀝니다.");
            ui.add_space(8.0);
            let (ok, cancel) = buttons_row(ui, "추가", 90.0);
            if ok && !d.sources.is_empty() {
                if let Some(l) = self.listing.clone() {
                    self.actions.push(Action::Start(JobKind::Add {
                        listing: l,
                        sources: d.sources.clone(),
                        base: self.cur_dir.clone(),
                        opts: ZipOptions { level: d.level, password: (d.use_password && !d.password.is_empty()).then(|| d.password.clone()) },
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
            ui.heading("삭제 확인");
            ui.add_space(4.0);
            let first = sel.first().map(|s| s.rsplit('/').next().unwrap_or(s).to_string()).unwrap_or_default();
            if sel.len() == 1 {
                ui.label(format!("'{first}'을(를) 압축 파일에서 삭제할까요?"));
            } else {
                ui.label(format!("'{first}' 외 {}개 항목을 압축 파일에서 삭제할까요?", sel.len() - 1));
            }
            ui.weak("삭제한 항목은 되돌릴 수 없습니다.");
            ui.add_space(8.0);
            let (ok, cancel) = buttons_row(ui, "삭제", 80.0);
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
            ui.heading("암호 입력");
            ui.add_space(4.0);
            if wrong {
                ui.colored_label(icons::RED, "암호가 올바르지 않습니다. 다시 입력하세요.");
            } else {
                ui.label("이 압축 파일은 암호로 보호되어 있습니다.");
            }
            let r = ui.add(egui::TextEdit::singleline(text).password(true).desired_width(300.0).hint_text("암호"));
            if text.is_empty() && !r.has_focus() {
                r.request_focus();
            }
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(8.0);
            let (ok, cancel) = buttons_row(ui, "확인", 80.0);
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
            ui.heading("끌어다 놓은 파일");
            ui.label(format!("항목 {}개를 어떻게 할까요?", paths.len()));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("지금 연 ZIP에 추가").clicked() {
                    self.actions.push(Action::ShowAdd(paths.clone()));
                    keep = false;
                }
                if ui.button("새로 압축하기").clicked() {
                    self.actions.push(Action::NewArchive(paths.clone()));
                    keep = false;
                }
                if ui.button("취소").clicked() {
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
            ui.add(egui::Label::new(if cur.is_empty() { "준비 중...".to_string() } else { cur }).truncate());
            ui.add(egui::ProgressBar::new(progress.fraction()).show_percentage().animate(true));
            let done = progress.done.load(Ordering::Relaxed);
            let total = progress.total.load(Ordering::Relaxed);
            ui.weak(format!("{} / {}", engine::format_size(done), engine::format_size(total)));
            ui.add_space(6.0);
            if ui.add_enabled(!progress.cancelled(), egui::Button::new("취소").min_size(vec2(70.0, 26.0))).clicked() {
                progress.cancel.store(true, Ordering::Relaxed);
            }
        });
    }
}

fn message_dialog(ctx: &egui::Context, title: &str, text: &str, error: bool) -> bool {
    let mut keep = true;
    let m = egui::Modal::new(egui::Id::unique("dlg_message")).show(ctx, |ui| {
        ui.set_max_width(480.0);
        ui.heading(RichText::new(title).color(if error { icons::RED } else { Color32::from_gray(20) }));
        ui.add_space(6.0);
        ui.label(text);
        ui.add_space(8.0);
        if ui.add(egui::Button::new("확인").min_size(vec2(80.0, 28.0))).clicked() {
            keep = false;
        }
    });
    if m.should_close() {
        keep = false;
    }
    keep
}
