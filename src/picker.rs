//! A small built-in file/folder chooser (works the same on every OS).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use egui::{Color32, RichText};

use crate::engine::format_size;
use crate::icons;

const LIST_HEIGHT: f32 = 340.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickPurpose {
    /// One archive file to open.
    OpenArchive,
    /// A destination folder for extraction.
    ExtractDest,
    /// Files and folders to compress into a new archive.
    CreateSources,
    /// Where to save a new archive (folder + file name).
    CreateDest,
    /// Files and folders to add to the open archive.
    AddSources,
}

impl PickPurpose {
    fn title(self) -> &'static str {
        match self {
            PickPurpose::OpenArchive => tr!("압축 파일 열기", "Open archive"),
            PickPurpose::ExtractDest => tr!("압축을 풀 폴더 선택", "Choose a folder to extract to"),
            PickPurpose::CreateSources => tr!("압축할 파일과 폴더 선택", "Choose files and folders to compress"),
            PickPurpose::CreateDest => tr!("압축 파일 저장 위치", "Save archive as"),
            PickPurpose::AddSources => tr!("추가할 파일과 폴더 선택", "Choose files and folders to add"),
        }
    }
    fn multi(self) -> bool {
        matches!(self, PickPurpose::CreateSources | PickPurpose::AddSources)
    }
    fn folder_only(self) -> bool {
        matches!(self, PickPurpose::ExtractDest)
    }
    fn wants_name(self) -> bool {
        self == PickPurpose::CreateDest
    }
}

#[derive(Debug, Clone)]
struct Item {
    name: String,
    path: PathBuf,
    is_dir: bool,
    size: u64,
    modified: String,
}

pub enum PickResult {
    Paths(PickPurpose, Vec<PathBuf>),
    Cancelled,
}

pub struct Picker {
    pub purpose: PickPurpose,
    dir: PathBuf,
    dir_text: String,
    items: Vec<Item>,
    selected: BTreeSet<PathBuf>,
    pub file_name: String,
    error: Option<String>,
    show_hidden: bool,
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn first_existing(base: &Path, names: &[&str]) -> Option<PathBuf> {
    names.iter().map(|n| base.join(n)).find(|p| p.is_dir())
}

/// Common places shown on the left of the chooser.
fn places() -> Vec<(String, PathBuf)> {
    let home = home_dir();
    let mut v = vec![(tr!("홈", "Home").to_string(), home.clone())];
    if let Some(p) = first_existing(&home, &["Desktop", "바탕화면", "바탕 화면"]) {
        v.push((tr!("바탕 화면", "Desktop").into(), p));
    }
    if let Some(p) = first_existing(&home, &["Documents", "문서"]) {
        v.push((tr!("문서", "Documents").into(), p));
    }
    if let Some(p) = first_existing(&home, &["Downloads", "다운로드"]) {
        v.push((tr!("다운로드", "Downloads").into(), p));
    }
    if cfg!(windows) {
        for letter in b'A'..=b'Z' {
            let d = PathBuf::from(format!("{}:\\", letter as char));
            if d.is_dir() {
                v.push((trf!("드라이브 {}:", "Drive {}:", letter as char), d));
            }
        }
    } else {
        v.push((tr!("컴퓨터 (/)", "Computer (/)").into(), PathBuf::from("/")));
    }
    v
}

impl Picker {
    pub fn new(purpose: PickPurpose, start: Option<PathBuf>, file_name: String) -> Self {
        let dir = start.filter(|p| p.is_dir()).unwrap_or_else(home_dir);
        let mut p = Self {
            purpose,
            dir: dir.clone(),
            dir_text: dir.display().to_string(),
            items: Vec::new(),
            selected: BTreeSet::new(),
            file_name,
            error: None,
            show_hidden: false,
        };
        p.refresh();
        p
    }

    fn go(&mut self, dir: PathBuf) {
        self.dir = dir;
        self.dir_text = self.dir.display().to_string();
        self.selected.clear();
        self.refresh();
    }

    fn refresh(&mut self) {
        self.items.clear();
        self.error = None;
        match std::fs::read_dir(&self.dir) {
            Ok(rd) => {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if !self.show_hidden && name.starts_with('.') {
                        continue;
                    }
                    let path = e.path();
                    let meta = std::fs::metadata(&path).ok();
                    let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
                    if self.purpose.folder_only() && !is_dir {
                        continue;
                    }
                    let modified = meta
                        .as_ref()
                        .and_then(|m| m.modified().ok())
                        .map(|t| DateTime::<Local>::from(t).format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_default();
                    self.items.push(Item {
                        name,
                        path,
                        is_dir,
                        size: meta.map(|m| m.len()).unwrap_or(0),
                        modified,
                    });
                }
                self.items.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
            }
            Err(e) => self.error = Some(trf!("폴더를 열 수 없습니다: {}", "Cannot open the folder: {}", crate::i18n::describe_error(&e.into()))),
        }
    }

    /// Draws the chooser as a modal; returns a result once the person decides.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<PickResult> {
        let mut result = None;
        let modal = egui::Modal::new(egui::Id::unique("rustzip_picker")).show(ctx, |ui| {
            ui.set_width(720.0);
            ui.heading(self.purpose.title());
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button(tr!("⬆ 상위 폴더", "⬆ Up")).clicked() {
                    if let Some(parent) = self.dir.parent().map(Path::to_path_buf) {
                        self.go(parent);
                    }
                }
                ui.label(tr!("위치:", "Location:"));
                let r = ui.add(egui::TextEdit::singleline(&mut self.dir_text).desired_width(470.0));
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) || ui.button(tr!("이동", "Go")).clicked() {
                    let p = PathBuf::from(self.dir_text.trim());
                    if p.is_dir() {
                        self.go(p);
                    } else {
                        self.error = Some(tr!("그런 폴더가 없습니다.", "That folder does not exist.").into());
                    }
                }
            });
            ui.separator();
            let mut go_to: Option<PathBuf> = None;
            let mut double_file: Option<PathBuf> = None;
            ui.horizontal_top(|ui| {
                ui.set_max_height(LIST_HEIGHT);
                ui.vertical(|ui| {
                    ui.set_width(130.0);
                    for (label, path) in places() {
                        if ui.selectable_label(self.dir == path, label).clicked() {
                            go_to = Some(path);
                        }
                    }
                    ui.add_space(8.0);
                    if ui.checkbox(&mut self.show_hidden, tr!("숨김 파일 보기", "Show hidden files")).changed() {
                        self.refresh();
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    egui::ScrollArea::vertical().max_height(LIST_HEIGHT).min_scrolled_height(LIST_HEIGHT).auto_shrink([false, false]).show(ui, |ui| {
                        if let Some(err) = &self.error {
                            ui.colored_label(Color32::from_rgb(200, 40, 40), err);
                        }
                        if self.items.is_empty() && self.error.is_none() {
                            ui.weak(tr!("(비어 있음)", "(empty)"));
                        }
                        for item in &self.items {
                            let sel = self.selected.contains(&item.path);
                            let row = ui.horizontal(|ui| {
                                let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                                if item.is_dir {
                                    icons::folder(ui.painter(), rect);
                                } else {
                                    icons::file(ui.painter(), rect, &item.name);
                                }
                                let r = ui.add_sized(
                                    [360.0, 20.0],
                                    egui::Button::selectable(sel, RichText::new(&item.name)).truncate(),
                                );
                                ui.weak(if item.is_dir { String::new() } else { format_size(item.size) });
                                ui.weak(&item.modified);
                                r
                            });
                            let r = row.inner;
                            if r.double_clicked() {
                                if item.is_dir {
                                    go_to = Some(item.path.clone());
                                } else {
                                    double_file = Some(item.path.clone());
                                }
                            } else if r.clicked() {
                                let multi_key = ui.input(|i| i.modifiers.command || i.modifiers.shift);
                                if self.purpose.multi() && multi_key {
                                    if sel {
                                        self.selected.remove(&item.path);
                                    } else {
                                        self.selected.insert(item.path.clone());
                                    }
                                } else {
                                    self.selected.clear();
                                    self.selected.insert(item.path.clone());
                                    if self.purpose.wants_name() && !item.is_dir {
                                        self.file_name = item.name.clone();
                                    }
                                }
                            }
                        }
                    });
                });
            });
            if let Some(p) = go_to {
                self.go(p);
            }
            ui.separator();
            if self.purpose.wants_name() {
                ui.horizontal(|ui| {
                    ui.label(tr!("파일 이름:", "File name:"));
                    ui.add(egui::TextEdit::singleline(&mut self.file_name).desired_width(400.0));
                });
            }
            if self.purpose.multi() {
                ui.weak(tr!("Ctrl 또는 Shift를 누른 채 누르면 여러 개를 고를 수 있습니다.", "Hold Ctrl or Shift to choose several items."));
            }
            ui.horizontal(|ui| {
                let ok_label = match self.purpose {
                    PickPurpose::OpenArchive => tr!("열기", "Open"),
                    PickPurpose::ExtractDest => tr!("이 폴더 선택", "Choose this folder"),
                    PickPurpose::CreateDest => tr!("저장", "Save"),
                    _ => tr!("선택", "Choose"),
                };
                let ok = ui.add(egui::Button::new(RichText::new(ok_label).strong()).min_size(egui::vec2(90.0, 26.0))).clicked();
                let cancel = ui.add(egui::Button::new(tr!("취소", "Cancel")).min_size(egui::vec2(70.0, 26.0))).clicked();
                if cancel {
                    result = Some(PickResult::Cancelled);
                }
                if ok || double_file.is_some() {
                    result = self.finish(double_file.take());
                }
            });
        });
        if modal.should_close() && result.is_none() {
            result = Some(PickResult::Cancelled);
        }
        result
    }

    fn finish(&mut self, double: Option<PathBuf>) -> Option<PickResult> {
        let purpose = self.purpose;
        match purpose {
            PickPurpose::OpenArchive => {
                let pick = double.or_else(|| self.selected.iter().find(|p| p.is_file()).cloned());
                match pick {
                    Some(p) => Some(PickResult::Paths(purpose, vec![p])),
                    None => {
                        self.error = Some(tr!("열 압축 파일을 고르세요.", "Choose an archive to open.").into());
                        None
                    }
                }
            }
            PickPurpose::ExtractDest => {
                let dir = self.selected.iter().next().cloned().filter(|p| p.is_dir()).unwrap_or_else(|| self.dir.clone());
                Some(PickResult::Paths(purpose, vec![dir]))
            }
            PickPurpose::CreateDest => {
                let name = self.file_name.trim();
                if name.is_empty() {
                    self.error = Some(tr!("파일 이름을 입력하세요.", "Enter a file name.").into());
                    return None;
                }
                Some(PickResult::Paths(purpose, vec![self.dir.join(name)]))
            }
            PickPurpose::CreateSources | PickPurpose::AddSources => {
                let mut v: Vec<PathBuf> = self.selected.iter().cloned().collect();
                if v.is_empty() {
                    if let Some(d) = double {
                        v.push(d);
                    }
                }
                if v.is_empty() {
                    self.error = Some(tr!("파일이나 폴더를 하나 이상 고르세요.", "Choose at least one file or folder.").into());
                    None
                } else {
                    Some(PickResult::Paths(purpose, v))
                }
            }
        }
    }
}
