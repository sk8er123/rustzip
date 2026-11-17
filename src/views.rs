//! Drawing the main window: menu, toolbar, location bar, folder tree, file list, status bar.

use std::path::PathBuf;

use egui::{pos2, vec2, Align, Color32, Layout, RichText, Sense, Stroke, StrokeKind};
use egui_extras::{Column, TableBuilder};

use crate::app::{Action, HudSeen, RustZipApp, SortCol};
use crate::engine::{self, NameEncoding};
use crate::icons::{self, Tool};

impl RustZipApp {
    pub fn frame_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.frame += 1;
        self.hud = HudSeen::default();
        self.poll_job();
        self.handle_drops(&ctx);
        self.shortcuts(&ctx);

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::top("toolbar")
            .frame(egui::Frame::new().fill(Color32::from_rgb(0xF4, 0xF7, 0xFB)).inner_margin(egui::Margin::symmetric(6, 4)))
            .show(ui, |ui| self.toolbar(ui));
        egui::Panel::top("pathbar")
            .frame(egui::Frame::new().fill(Color32::from_rgb(0xFA, 0xFB, 0xFD)).inner_margin(egui::Margin::symmetric(8, 4)))
            .show(ui, |ui| self.path_bar(ui));
        egui::Panel::bottom("status")
            .frame(egui::Frame::new().fill(Color32::from_rgb(0xEE, 0xF1, 0xF5)).inner_margin(egui::Margin::symmetric(8, 3)))
            .show(ui, |ui| self.status_bar(ui));
        if self.listing.is_some() {
            egui::Panel::left("tree")
                .resizable(true)
                .default_size(210.0)
                .size_range(120.0..=420.0)
                .show(ui, |ui| self.tree(ui));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::WHITE).inner_margin(0))
            .show(ui, |ui| {
                if self.listing.is_some() {
                    self.file_table(ui);
                } else {
                    self.empty_state(ui);
                }
            });

        self.drag_overlay(&ctx);
        self.dialogs(&ctx);
        self.progress_modal(&ctx);
        self.process_actions(&ctx);
        if self.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| Some(f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty())).collect());
        if !dropped.is_empty() {
            self.actions.push(Action::Dropped(dropped));
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() || self.picker.is_some() || self.busy() {
            return;
        }
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        let (o, n, e, del, back, enter, all) = ctx.input(|i| {
            let c = i.modifiers.command;
            (
                c && i.key_pressed(egui::Key::O),
                c && i.key_pressed(egui::Key::N),
                c && i.key_pressed(egui::Key::E),
                i.key_pressed(egui::Key::Delete),
                i.key_pressed(egui::Key::Backspace),
                i.key_pressed(egui::Key::Enter),
                c && i.key_pressed(egui::Key::A),
            )
        });
        if o {
            self.actions.push(Action::PickOpen);
        }
        if n {
            self.actions.push(Action::NewArchive(Vec::new()));
        }
        if self.listing.is_none() {
            return;
        }
        if e {
            self.actions.push(Action::ShowExtract);
        }
        if del && self.editable() && !self.selected.is_empty() {
            self.actions.push(Action::AskDelete);
        }
        if back && !self.cur_dir.is_empty() {
            self.actions.push(Action::Up);
        }
        if all {
            self.selected = self.view.iter().map(|c| c.path.clone()).collect();
        }
        if enter && self.selected.len() == 1 {
            let p = self.selected.iter().next().cloned().unwrap_or_default();
            if let Some(c) = self.view.iter().find(|c| c.path == p) {
                self.actions.push(if c.is_dir { Action::Navigate(p) } else { Action::OpenEntry(p) });
            }
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let has = self.listing.is_some();
        let editable = self.editable();
        let has_sel = !self.selected.is_empty();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button(tr!("파일(F)", "File"), |ui| {
                if ui.add(egui::Button::new(tr!("압축 파일 열기...", "Open archive...")).shortcut_text("Ctrl+O")).clicked() {
                    self.actions.push(Action::PickOpen);
                }
                if ui.add(egui::Button::new(tr!("새로 압축하기...", "New archive...")).shortcut_text("Ctrl+N")).clicked() {
                    self.actions.push(Action::NewArchive(Vec::new()));
                }
                if ui.add_enabled(has, egui::Button::new(tr!("압축 파일 닫기", "Close archive"))).clicked() {
                    self.actions.push(Action::Close);
                }
                ui.separator();
                if ui.button(tr!("끝내기", "Exit")).clicked() {
                    self.actions.push(Action::Quit);
                }
            });
            ui.menu_button(tr!("작업(A)", "Actions"), |ui| {
                if ui.add_enabled(has, egui::Button::new(tr!("압축 풀기...", "Extract...")).shortcut_text("Ctrl+E")).clicked() {
                    self.actions.push(Action::ShowExtract);
                }
                if ui.add_enabled(editable, egui::Button::new(tr!("파일 추가...", "Add files..."))).clicked() {
                    self.actions.push(Action::ShowAdd(Vec::new()));
                }
                if ui.add_enabled(editable && has_sel, egui::Button::new(tr!("선택 항목 삭제", "Delete selected")).shortcut_text("Del")).clicked() {
                    self.actions.push(Action::AskDelete);
                }
                ui.separator();
                if ui.add_enabled(has, egui::Button::new(tr!("모두 선택", "Select all")).shortcut_text("Ctrl+A")).clicked() {
                    self.selected = self.view.iter().map(|c| c.path.clone()).collect();
                }
                if ui.add_enabled(has && !self.cur_dir.is_empty(), egui::Button::new(tr!("상위 폴더로", "Up one folder")).shortcut_text("Backspace")).clicked() {
                    self.actions.push(Action::Up);
                }
            });
            ui.menu_button(tr!("보기(V)", "View"), |ui| {
                ui.label(RichText::new(tr!("파일 이름 인코딩", "File name encoding")).strong());
                for enc in NameEncoding::ALL {
                    if ui.radio(self.encoding == enc, enc.label()).clicked() {
                        self.actions.push(Action::SetEncoding(enc));
                    }
                }
                ui.separator();
                // Shown in both languages so it can be found whichever is active.
                ui.label(RichText::new("언어 / Language").strong());
                let current = crate::i18n::lang();
                for lang in crate::i18n::Lang::ALL {
                    if ui.radio(current == lang, lang.native_name()).clicked() {
                        self.actions.push(Action::SetLanguage(lang));
                    }
                }
            });
            ui.menu_button(tr!("도움말(H)", "Help"), |ui| {
                if ui.button(tr!("RustZip 정보...", "About RustZip...")).clicked() {
                    self.actions.push(Action::About);
                }
            });
        });
    }

    fn tool_button(&mut self, ui: &mut egui::Ui, tool: Tool, label: &str, enabled: bool, tip: &str) -> bool {
        let (rect, resp) = ui.allocate_exact_size(vec2(70.0, 58.0), if enabled { Sense::click() } else { Sense::hover() });
        let p = ui.painter();
        if enabled && resp.hovered() {
            p.rect(rect, 5, Color32::from_rgb(0xDD, 0xEA, 0xFB), Stroke::new(1.0, Color32::from_rgb(0x9C, 0xC0, 0xF0)), StrokeKind::Inside);
        }
        let icon = egui::Rect::from_center_size(pos2(rect.center().x, rect.top() + 20.0), vec2(32.0, 32.0));
        icons::tool(p, icon, tool, enabled);
        let color = if enabled { Color32::from_gray(30) } else { Color32::from_gray(160) };
        p.text(pos2(rect.center().x, rect.bottom() - 9.0), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(12.5), color);
        let resp = resp.on_hover_text(tip);
        enabled && resp.clicked()
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        self.hud.toolbar = true;
        let has = self.listing.is_some() && !self.busy();
        let editable = self.editable() && !self.busy();
        let idle = !self.busy();
        let has_sel = !self.selected.is_empty();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if self.tool_button(ui, Tool::Open, tr!("열기", "Open"), idle, tr!("압축 파일 열기 (Ctrl+O)", "Open an archive (Ctrl+O)")) {
                self.actions.push(Action::PickOpen);
            }
            if self.tool_button(ui, Tool::New, tr!("새로 압축", "New"), idle, tr!("파일과 폴더로 새 ZIP 만들기 (Ctrl+N)", "Make a new ZIP from files and folders (Ctrl+N)")) {
                self.actions.push(Action::NewArchive(Vec::new()));
            }
            if self.tool_button(ui, Tool::Extract, tr!("압축 풀기", "Extract"), has, tr!("모두 또는 선택한 파일 풀기 (Ctrl+E)", "Extract all or the selected files (Ctrl+E)")) {
                self.actions.push(Action::ShowExtract);
            }
            ui.add(egui::Separator::default().vertical().spacing(10.0));
            if self.tool_button(ui, Tool::Add, tr!("추가", "Add"), editable, tr!("열린 ZIP에 파일 추가", "Add files to the open ZIP")) {
                self.actions.push(Action::ShowAdd(Vec::new()));
            }
            if self.tool_button(ui, Tool::Delete, tr!("삭제", "Delete"), editable && has_sel, tr!("선택한 항목을 ZIP에서 삭제 (Del)", "Delete the selected items from the ZIP (Del)")) {
                self.actions.push(Action::AskDelete);
            }
            if self.tool_button(ui, Tool::View, tr!("파일 열기", "View"), has && self.selected.len() == 1, tr!("선택한 파일을 연결된 프로그램으로 열기", "Open the selected file with its default program")) {
                if let Some(p) = self.selected.iter().next().cloned() {
                    let is_dir = self.view.iter().any(|c| c.path == p && c.is_dir);
                    self.actions.push(if is_dir { Action::Navigate(p) } else { Action::OpenEntry(p) });
                }
            }
            ui.add(egui::Separator::default().vertical().spacing(10.0));
            if self.tool_button(ui, Tool::Up, tr!("상위 폴더", "Up"), has && !self.cur_dir.is_empty(), tr!("상위 폴더로 (Backspace)", "Up one folder (Backspace)")) {
                self.actions.push(Action::Up);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.tool_button(ui, Tool::Info, tr!("정보", "About"), true, tr!("RustZip 정보", "About RustZip")) {
                    self.actions.push(Action::About);
                }
            });
        });
    }

    fn path_bar(&mut self, ui: &mut egui::Ui) {
        self.hud.path_bar = true;
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr!("위치", "Location")).color(Color32::from_gray(90)));
            let Some(l) = self.listing.clone() else {
                ui.weak(tr!("열린 압축 파일이 없습니다", "No archive is open"));
                return;
            };
            let arch = l.path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let mut enc = self.encoding;
                egui::ComboBox::from_id_salt("enc").selected_text(enc.label()).width(170.0).show_ui(ui, |ui| {
                    for e in NameEncoding::ALL {
                        ui.selectable_value(&mut enc, e, e.label());
                    }
                });
                if enc != self.encoding {
                    self.actions.push(Action::SetEncoding(enc));
                }
                ui.label(tr!("이름 인코딩", "Name encoding"));
                ui.add_space(6.0);
                if l.any_encrypted() {
                    ui.label(RichText::new(tr!(" 암호 ", " Encrypted ")).color(Color32::WHITE).background_color(Color32::from_rgb(0xC0, 0x8A, 0x10)));
                }
                ui.label(RichText::new(format!(" {} ", l.kind.label())).color(Color32::WHITE).background_color(icons::ACCENT).strong());
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    egui::Frame::new()
                        .fill(Color32::WHITE)
                        .stroke(Stroke::new(1.0, Color32::from_gray(200)))
                        .corner_radius(3)
                        .inner_margin(egui::Margin::symmetric(6, 2))
                        .show(ui, |ui| {
                            ui.set_width((ui.available_width() - 12.0).max(50.0));
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 3.0;
                                let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                                icons::archive_box(ui.painter(), r.shrink(1.0), true);
                                if ui.link(RichText::new(&arch).strong()).clicked() {
                                    self.actions.push(Action::Navigate(String::new()));
                                }
                                let mut acc = String::new();
                                for part in self.cur_dir.split('/').filter(|s| !s.is_empty()) {
                                    ui.weak("›");
                                    if !acc.is_empty() {
                                        acc.push('/');
                                    }
                                    acc.push_str(part);
                                    if ui.link(part).clicked() {
                                        self.actions.push(Action::Navigate(acc.clone()));
                                    }
                                }
                            });
                        });
                });
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        self.hud.status = true;
        ui.horizontal(|ui| {
            if let Some(job) = &self.job {
                ui.add(egui::Spinner::new());
                ui.add(egui::ProgressBar::new(job.progress.fraction()).desired_width(180.0).show_percentage());
            }
            ui.label(&self.status);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(l) = &self.listing {
                    let dirs = self.view.iter().filter(|c| c.is_dir).count();
                    let files = self.view.len() - dirs;
                    let sel_size: u64 = self.view.iter().filter(|c| self.selected.contains(&c.path)).map(|c| c.size).sum();
                    ui.label(trf!("압축 파일 {}", "Archive {}", engine::format_size(l.archive_size)));
                    ui.separator();
                    if !self.selected.is_empty() {
                        ui.label(trf!("{}개 선택 ({})", "{} selected ({})", self.selected.len(), engine::format_size(sel_size)));
                        ui.separator();
                    }
                    ui.label(trf!("폴더 {dirs}개, 파일 {files}개", "{dirs} folder(s), {files} file(s)"));
                }
            });
        });
    }

    fn tree(&mut self, ui: &mut egui::Ui) {
        self.hud.tree = true;
        let Some(l) = self.listing.clone() else { return };
        ui.add_space(4.0);
        ui.label(RichText::new(tr!("폴더", "Folders")).strong().color(Color32::from_gray(80)));
        ui.separator();
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            let arch = l.path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let folders = self.folders.clone();
            self.tree_row(ui, "", &arch, 0, !folders.is_empty());
            for f in &folders {
                if !self.ancestors_expanded(&engine::parent_dir(f)) {
                    continue;
                }
                let depth = f.matches('/').count() + 1;
                let name = f.rsplit('/').next().unwrap_or(f).to_string();
                let has_kids = folders.iter().any(|o| o.len() > f.len() && o.starts_with(f.as_str()) && o.as_bytes()[f.len()] == b'/');
                self.tree_row(ui, f, &name, depth, has_kids);
            }
        });
    }

    fn ancestors_expanded(&self, dir: &str) -> bool {
        let mut d = dir;
        loop {
            if !self.expanded.contains(d) {
                return false;
            }
            if d.is_empty() {
                return true;
            }
            d = d.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        }
    }

    fn toggle(&mut self, path: &str) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_string());
        }
    }

    fn tree_row(&mut self, ui: &mut egui::Ui, path: &str, name: &str, depth: usize, has_kids: bool) {
        let width = ui.available_width().max(160.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(width, 21.0), Sense::click());
        let p = ui.painter();
        let current = self.cur_dir == path;
        if current {
            p.rect_filled(rect, 3, Color32::from_rgb(0xD5, 0xE6, 0xFC));
        } else if resp.hovered() {
            p.rect_filled(rect, 3, Color32::from_rgb(0xEE, 0xF4, 0xFD));
        }
        let x = rect.left() + 4.0 + depth as f32 * 14.0;
        let open = self.expanded.contains(path);
        let tri = egui::Rect::from_min_size(pos2(x, rect.top() + 5.5), vec2(10.0, 10.0));
        if has_kids {
            let c = tri.center();
            let pts = if open {
                vec![c + vec2(-4.0, -2.0), c + vec2(4.0, -2.0), c + vec2(0.0, 3.0)]
            } else {
                vec![c + vec2(-2.0, -4.0), c + vec2(3.0, 0.0), c + vec2(-2.0, 4.0)]
            };
            p.add(egui::Shape::convex_polygon(pts, Color32::from_gray(110), Stroke::NONE));
        }
        let icon = egui::Rect::from_min_size(pos2(x + 13.0, rect.top() + 2.5), vec2(17.0, 16.0));
        if depth == 0 {
            icons::archive_box(p, icon.shrink(1.0), true);
        } else {
            icons::folder(p, icon);
        }
        p.text(
            pos2(icon.right() + 5.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            name,
            egui::FontId::proportional(13.5),
            if current { icons::ACCENT_DARK } else { Color32::from_gray(30) },
        );
        let on_triangle = resp.interact_pointer_pos().is_some_and(|pp| pp.x < tri.right() + 3.0);
        if (resp.clicked() && on_triangle || resp.double_clicked()) && has_kids {
            self.toggle(path);
        } else if resp.clicked() {
            self.actions.push(Action::Navigate(path.to_string()));
        }
    }

    fn header_cell(&mut self, ui: &mut egui::Ui, label: &'static str, col: SortCol) {
        self.hud.columns.push(label);
        let arrow = match (self.sort.0 == col, self.sort.1) {
            (true, true) => " ▲",
            (true, false) => " ▼",
            _ => "",
        };
        let r = ui.add(egui::Label::new(RichText::new(format!("{label}{arrow}")).strong()).sense(Sense::click()).selectable(false));
        if r.clicked() {
            if self.sort.0 == col {
                self.sort.1 = !self.sort.1;
            } else {
                self.sort = (col, true);
            }
            self.refresh_view();
        }
    }

    fn file_table(&mut self, ui: &mut egui::Ui) {
        let rows = self.view.clone();
        let mut clicked: Option<(usize, egui::Modifiers)> = None;
        let mut double: Option<usize> = None;
        let mut context_on: Option<usize> = None;
        let has_parent = !self.cur_dir.is_empty();
        let offset = usize::from(has_parent);
        let row_count = rows.len() + offset;
        self.hud.rows = rows.len();
        let editable = self.editable();
        let selected = self.selected.clone();
        let mut menu_actions: Vec<Action> = Vec::new();

        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::remainder().at_least(220.0).clip(true))
            .column(Column::initial(90.0).at_least(60.0))
            .column(Column::initial(90.0).at_least(60.0))
            .column(Column::initial(70.0).at_least(50.0))
            .column(Column::initial(100.0).at_least(60.0).clip(true))
            .column(Column::initial(140.0).at_least(100.0))
            .auto_shrink([false, false])
            .header(24.0, |mut header| {
                header.col(|ui| self.header_cell(ui, tr!("이름", "Name"), SortCol::Name));
                header.col(|ui| self.header_cell(ui, tr!("크기", "Size"), SortCol::Size));
                header.col(|ui| self.header_cell(ui, tr!("압축 크기", "Packed"), SortCol::Packed));
                header.col(|ui| self.header_cell(ui, tr!("압축률", "Ratio"), SortCol::Ratio));
                header.col(|ui| self.header_cell(ui, tr!("압축 방식", "Method"), SortCol::Method));
                header.col(|ui| self.header_cell(ui, tr!("수정한 날짜", "Modified"), SortCol::Modified));
            })
            .body(|body| {
                body.rows(22.0, row_count, |mut row| {
                    let i = row.index();
                    if has_parent && i == 0 {
                        row.col(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                            icons::folder(ui.painter(), r);
                            ui.add(egui::Label::new("..").selectable(false));
                        });
                        row.col(|ui| {
                            ui.add(egui::Label::new(RichText::new(tr!("상위 폴더", "Parent folder")).weak()).selectable(false));
                        });
                        row.col(|_| {});
                        row.col(|_| {});
                        row.col(|_| {});
                        row.col(|_| {});
                        if row.response().double_clicked() {
                            double = Some(usize::MAX);
                        }
                        return;
                    }
                    let idx = i - offset;
                    let c = &rows[idx];
                    row.set_selected(selected.contains(&c.path));
                    row.col(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                        if c.is_dir {
                            icons::folder(ui.painter(), r);
                        } else {
                            icons::file(ui.painter(), r, &c.name);
                        }
                        ui.add(egui::Label::new(&c.name).truncate().selectable(false));
                        if c.encrypted {
                            let (r, _) = ui.allocate_exact_size(vec2(12.0, 14.0), Sense::hover());
                            icons::lock(ui.painter(), r);
                        }
                    });
                    let right = |ui: &mut egui::Ui, s: String| {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.add(egui::Label::new(s).selectable(false));
                        });
                    };
                    row.col(|ui| right(ui, if c.is_dir && c.file_count == 0 { String::new() } else { engine::format_size(c.size) }));
                    row.col(|ui| right(ui, c.packed.map(engine::format_size).unwrap_or_else(|| "-".into())));
                    row.col(|ui| right(ui, engine::ratio(c.size, c.packed).map(|r| format!("{r:.0}%")).unwrap_or_else(|| "-".into())));
                    row.col(|ui| {
                        ui.add(egui::Label::new(c.method.as_str()).truncate().selectable(false));
                    });
                    row.col(|ui| {
                        let m = c.modified.map(|m| m.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default();
                        ui.add(egui::Label::new(m).selectable(false));
                    });
                    let resp = row.response();
                    if resp.double_clicked() {
                        double = Some(idx);
                    } else if resp.clicked() {
                        clicked = Some((idx, resp.ctx.input(|i| i.modifiers)));
                    } else if resp.secondary_clicked() {
                        context_on = Some(idx);
                    }
                    resp.context_menu(|ui| {
                        if c.is_dir {
                            if ui.button(tr!("폴더 열기", "Open folder")).clicked() {
                                menu_actions.push(Action::Navigate(c.path.clone()));
                                ui.close();
                            }
                        } else if ui.button(tr!("열기", "Open")).clicked() {
                            menu_actions.push(Action::OpenEntry(c.path.clone()));
                            ui.close();
                        }
                        if ui.button(tr!("압축 풀기...", "Extract...")).clicked() {
                            menu_actions.push(Action::ShowExtract);
                            ui.close();
                        }
                        if ui.add_enabled(editable, egui::Button::new(tr!("삭제", "Delete"))).clicked() {
                            menu_actions.push(Action::AskDelete);
                            ui.close();
                        }
                    });
                });
            });
        self.actions.extend(menu_actions);

        if let Some(idx) = context_on {
            let p = rows[idx].path.clone();
            if !self.selected.contains(&p) {
                self.selected.clear();
                self.selected.insert(p);
            }
        }
        if let Some((idx, m)) = clicked {
            let p = rows[idx].path.clone();
            if m.shift {
                let a = self.anchor.unwrap_or(idx);
                let (lo, hi) = if a <= idx { (a, idx) } else { (idx, a) };
                if !m.command {
                    self.selected.clear();
                }
                for r in &rows[lo..=hi] {
                    self.selected.insert(r.path.clone());
                }
            } else if m.command {
                if !self.selected.remove(&p) {
                    self.selected.insert(p);
                }
                self.anchor = Some(idx);
            } else {
                self.selected.clear();
                self.selected.insert(p);
                self.anchor = Some(idx);
            }
        }
        if let Some(idx) = double {
            if idx == usize::MAX {
                self.actions.push(Action::Up);
            } else {
                let c = &rows[idx];
                self.actions.push(if c.is_dir { Action::Navigate(c.path.clone()) } else { Action::OpenEntry(c.path.clone()) });
            }
        }
    }

    fn empty_state(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap().shrink(24.0);
        let p = ui.painter().clone();
        p.rect(rect, 12, Color32::from_rgb(0xF7, 0xFA, 0xFE), Stroke::new(2.0, Color32::from_rgb(0xB9, 0xD0, 0xF2)), StrokeKind::Inside);
        let c = rect.center();
        icons::archive_box(&p, egui::Rect::from_center_size(c - vec2(0.0, 80.0), vec2(48.0, 48.0)), true);
        p.text(c - vec2(0.0, 24.0), egui::Align2::CENTER_CENTER, tr!("압축 파일이나 압축할 파일을 이 창으로 끌어다 놓으세요", "Drag an archive, or files to compress, into this window"), egui::FontId::proportional(19.0), Color32::from_gray(50));
        p.text(
            c + vec2(0.0, 6.0),
            egui::Align2::CENTER_CENTER,
            tr!("ZIP · 7Z · TAR · TAR.GZ · TAR.BZ2 · TAR.XZ · GZ 를 열 수 있고, ZIP으로 압축할 수 있습니다", "Opens ZIP · 7Z · TAR · TAR.GZ · TAR.BZ2 · TAR.XZ · GZ and compresses to ZIP"),
            egui::FontId::proportional(13.5),
            Color32::from_gray(110),
        );
        let btn_area = egui::Rect::from_center_size(c + vec2(0.0, 52.0), vec2(300.0, 34.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(btn_area).layout(Layout::left_to_right(Align::Center)));
        child.spacing_mut().item_spacing.x = 16.0;
        if child.add(egui::Button::new(RichText::new(tr!("압축 파일 열기", "Open archive")).size(15.0)).min_size(vec2(140.0, 32.0))).clicked() {
            self.actions.push(Action::PickOpen);
        }
        if child.add(egui::Button::new(RichText::new(tr!("새로 압축하기", "New archive")).size(15.0)).min_size(vec2(140.0, 32.0))).clicked() {
            self.actions.push(Action::NewArchive(Vec::new()));
        }
    }

    fn drag_overlay(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            return;
        }
        let screen = ctx.content_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("drop_overlay")));
        painter.rect_filled(screen, 0, Color32::from_rgba_unmultiplied(30, 111, 217, 60));
        painter.rect_stroke(screen.shrink(8.0), 10, Stroke::new(3.0, icons::ACCENT), StrokeKind::Inside);
        let msg = if self.editable() {
            tr!("놓으면 압축 파일을 열거나 지금 ZIP에 추가합니다", "Drop to open an archive or add to this ZIP")
        } else {
            tr!("놓으면 압축 파일을 열거나 새로 압축합니다", "Drop to open an archive or make a new one")
        };
        painter.text(screen.center(), egui::Align2::CENTER_CENTER, msg, egui::FontId::proportional(22.0), icons::ACCENT_DARK);
    }
}
