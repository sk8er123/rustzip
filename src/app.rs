//! RustZip main window: state, background jobs and actions.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::engine::{self, archive_error, ArchiveError, Child, ExtractOptions, ExtractReport, Listing, NameEncoding, Overwrite, Progress, ZipOptions};
use crate::picker::{PickPurpose, Picker};
use crate::selftest::SelfTest;

// ---------------------------------------------------------------------------
// Background jobs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum JobKind {
    Open { path: PathBuf, encoding: NameEncoding, password: Option<String> },
    Extract { listing: Arc<Listing>, opts: ExtractOptions, then: AfterExtract },
    Create { dest: PathBuf, sources: Vec<PathBuf>, opts: ZipOptions },
    Add { listing: Arc<Listing>, sources: Vec<PathBuf>, base: String, opts: ZipOptions },
    Delete { listing: Arc<Listing>, selection: Vec<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum AfterExtract {
    Nothing,
    OpenFolder,
    /// Open this extracted file with the system's default program.
    OpenFile(PathBuf),
}

impl JobKind {
    pub fn title(&self) -> &'static str {
        match self {
            JobKind::Open { .. } => tr!("압축 파일 여는 중", "Opening archive"),
            JobKind::Extract { then: AfterExtract::OpenFile(_), .. } => tr!("파일 여는 중", "Opening file"),
            JobKind::Extract { .. } => tr!("압축 푸는 중", "Extracting"),
            JobKind::Create { .. } => tr!("압축하는 중", "Compressing"),
            JobKind::Add { .. } => tr!("파일 추가하는 중", "Adding files"),
            JobKind::Delete { .. } => tr!("파일 삭제하는 중", "Deleting files"),
        }
    }
    pub fn set_password(&mut self, pw: String) {
        match self {
            JobKind::Open { password, .. } => *password = Some(pw),
            JobKind::Extract { opts, .. } => opts.password = Some(pw),
            _ => {}
        }
    }
}

pub enum JobOutcome {
    Opened(Listing),
    Extracted(ExtractReport, AfterExtract, PathBuf),
    Created(Listing, usize),
    Modified(Listing, String),
}

pub struct RunningJob {
    pub kind: JobKind,
    pub progress: Arc<Progress>,
    rx: Receiver<anyhow::Result<JobOutcome>>,
    pub started: Instant,
}

fn run_job(kind: JobKind, progress: &Progress) -> anyhow::Result<JobOutcome> {
    match kind {
        JobKind::Open { path, encoding, password } => {
            Ok(JobOutcome::Opened(engine::list_with_password(&path, encoding, password.as_deref(), progress)?))
        }
        JobKind::Extract { listing, opts, then } => {
            let dest = opts.dest.clone();
            let rep = engine::extract(&listing, &opts, progress)?;
            Ok(JobOutcome::Extracted(rep, then, dest))
        }
        JobKind::Create { dest, sources, opts } => {
            let n = engine::create_zip(&dest, &sources, &opts, progress)?;
            let l = engine::list(&dest, NameEncoding::Auto, &Progress::default())?;
            Ok(JobOutcome::Created(l, n))
        }
        JobKind::Add { listing, sources, base, opts } => {
            let n = engine::add_to_zip(&listing, &sources, &base, &opts, progress)?;
            let l = engine::list(&listing.path, listing.encoding, &Progress::default())?;
            Ok(JobOutcome::Modified(l, trf!("파일 {n}개를 추가했습니다.", "Added {n} file(s).")))
        }
        JobKind::Delete { listing, selection } => {
            let n = engine::delete_from_zip(&listing, &selection, progress)?;
            let l = engine::list(&listing.path, listing.encoding, &Progress::default())?;
            Ok(JobOutcome::Modified(l, trf!("항목 {n}개를 삭제했습니다.", "Deleted {n} item(s).")))
        }
    }
}

// ---------------------------------------------------------------------------
// Dialog state
// ---------------------------------------------------------------------------

pub struct ExtractDlg {
    pub dest: String,
    pub selected_only: bool,
    pub can_selected: bool,
    pub own_folder: bool,
    pub overwrite: Overwrite,
    pub open_after: bool,
}

pub struct CreateDlg {
    pub sources: Vec<PathBuf>,
    pub dest: String,
    pub level: u8,
    pub use_password: bool,
    pub password: String,
    pub password2: String,
    pub show_password: bool,
    pub error: Option<String>,
}

pub struct AddDlg {
    pub sources: Vec<PathBuf>,
    pub level: u8,
    pub use_password: bool,
    pub password: String,
    pub show_password: bool,
}

pub enum Dialog {
    Extract(ExtractDlg),
    Create(CreateDlg),
    Add(AddDlg),
    ConfirmDelete(Vec<String>),
    Password { job: JobKind, text: String, wrong: bool },
    Message { title: String, text: String, error: bool },
    DropChoice(Vec<PathBuf>),
    About,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortCol {
    Name,
    Size,
    Packed,
    Ratio,
    Modified,
}

/// What the person asked for this frame (handled after drawing).
pub enum Action {
    PickOpen,
    OpenPath(PathBuf),
    Close,
    NewArchive(Vec<PathBuf>),
    ShowExtract,
    ShowAdd(Vec<PathBuf>),
    AskDelete,
    Navigate(String),
    Up,
    OpenEntry(String),
    SetEncoding(NameEncoding),
    SetLanguage(crate::i18n::Lang),
    Start(JobKind),
    Dropped(Vec<PathBuf>),
    About,
    Quit,
}

/// Which parts of the screen were drawn in the last frame (read by the self-test).
#[derive(Default, Clone, Debug)]
pub struct HudSeen {
    pub toolbar: bool,
    pub columns: Vec<&'static str>,
    pub rows: usize,
    pub status: bool,
    pub tree: bool,
    pub path_bar: bool,
}

pub struct ScreenshotState {
    pub path: PathBuf,
    pub requested: bool,
    pub image: Arc<Mutex<Option<Arc<egui::ColorImage>>>>,
}

pub struct RustZipApp {
    pub listing: Option<Arc<Listing>>,
    pub encoding: NameEncoding,
    pub cur_dir: String,
    pub view: Vec<Child>,
    pub folders: Vec<String>,
    pub expanded: HashSet<String>,
    pub selected: BTreeSet<String>,
    pub anchor: Option<usize>,
    pub sort: (SortCol, bool),
    pub job: Option<RunningJob>,
    pub dialog: Option<Dialog>,
    pub picker: Option<Picker>,
    pub status: String,
    pub actions: Vec<Action>,
    pub hud: HudSeen,
    /// Password that worked for the open archive (reused for later extractions).
    pub known_password: Option<String>,
    pub last_extract_dir: Option<PathBuf>,
    pub screenshot: Option<ScreenshotState>,
    pub selftest: Option<SelfTest>,
    pub frame: u64,
    /// When true the app never opens folders/files in other programs or shows result pop-ups.
    pub quiet: bool,
}

impl RustZipApp {
    pub fn new(open: Option<PathBuf>) -> Self {
        let mut app = Self {
            listing: None,
            encoding: NameEncoding::Auto,
            cur_dir: String::new(),
            view: Vec::new(),
            folders: Vec::new(),
            expanded: HashSet::new(),
            selected: BTreeSet::new(),
            anchor: None,
            sort: (SortCol::Name, true),
            job: None,
            dialog: None,
            picker: None,
            status: tr!("준비", "Ready").into(),
            actions: Vec::new(),
            hud: HudSeen::default(),
            known_password: None,
            last_extract_dir: None,
            screenshot: None,
            selftest: None,
            frame: 0,
            quiet: false,
        };
        if let Some(p) = open {
            app.actions.push(Action::OpenPath(p));
        }
        app
    }

    /// Opens an archive immediately on this thread (used at start-up for screenshots).
    pub fn open_now(&mut self, path: &Path) -> anyhow::Result<()> {
        let l = engine::list(path, self.encoding, &Progress::default())?;
        self.set_listing(l);
        Ok(())
    }

    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    pub fn start_job(&mut self, kind: JobKind) {
        if self.job.is_some() {
            return;
        }
        let progress = Progress::new();
        let (tx, rx) = mpsc::channel();
        let p2 = progress.clone();
        let k2 = kind.clone();
        std::thread::spawn(move || {
            let r = run_job(k2, &p2);
            let _ = tx.send(r);
        });
        self.status = format!("{}...", kind.title());
        self.job = Some(RunningJob { kind, progress, rx, started: Instant::now() });
    }

    pub fn set_listing(&mut self, l: Listing) {
        let same = self.listing.as_ref().is_some_and(|old| old.path == l.path);
        self.encoding = l.encoding;
        self.folders = engine::all_folders(&l.entries);
        if !same {
            self.cur_dir.clear();
            self.expanded.clear();
            self.expanded.insert(String::new());
        } else if !self.cur_dir.is_empty() && !self.folders.contains(&self.cur_dir) {
            self.cur_dir.clear();
        }
        self.listing = Some(Arc::new(l));
        self.status = self.idle_status();
        self.selected.clear();
        self.refresh_view();
    }

    /// Status bar text when nothing is running: what is open, or "Ready".
    pub fn idle_status(&self) -> String {
        let Some(l) = &self.listing else { return tr!("준비", "Ready").into() };
        let name = l.path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let (kind, files, size) = (l.kind.label(), l.file_count(), engine::format_size(l.total_size()));
        trf!(
            "{name} 열림 — {kind} 형식, 파일 {files}개, 원래 크기 {size}",
            "{name} open — {kind} format, {files} file(s), {size} uncompressed"
        )
    }

    /// Switches the interface language and remembers the choice for next time.
    pub fn set_language(&mut self, lang: crate::i18n::Lang) {
        crate::i18n::set_lang(lang);
        if !self.quiet {
            let _ = crate::i18n::save_lang(lang);
        }
        if !self.busy() {
            self.status = self.idle_status();
        }
    }

    pub fn refresh_view(&mut self) {
        let Some(l) = &self.listing else {
            self.view.clear();
            return;
        };
        let mut v = engine::children(&l.entries, &self.cur_dir);
        let (col, asc) = self.sort;
        v.sort_by(|a, b| {
            let o = match col {
                SortCol::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortCol::Size => a.size.cmp(&b.size),
                SortCol::Packed => a.packed.cmp(&b.packed),
                SortCol::Ratio => engine::ratio(a.size, a.packed)
                    .unwrap_or(0.0)
                    .total_cmp(&engine::ratio(b.size, b.packed).unwrap_or(0.0)),
                SortCol::Modified => a.modified.cmp(&b.modified),
            };
            let o = if asc { o } else { o.reverse() };
            b.is_dir.cmp(&a.is_dir).then(o)
        });
        self.view = v;
        let view = &self.view;
        self.selected.retain(|s| view.iter().any(|c| &c.path == s));
        self.anchor = None;
    }

    pub fn navigate(&mut self, dir: String) {
        let mut d = dir.as_str();
        loop {
            self.expanded.insert(d.to_string());
            if d.is_empty() {
                break;
            }
            d = d.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        }
        self.cur_dir = dir;
        self.selected.clear();
        self.refresh_view();
    }

    pub fn selection_vec(&self) -> Vec<String> {
        self.selected.iter().cloned().collect()
    }

    pub fn editable(&self) -> bool {
        self.listing.as_ref().is_some_and(|l| l.kind.is_editable())
    }

    fn default_extract_dest(&self) -> PathBuf {
        self.last_extract_dir
            .clone()
            .or_else(|| self.listing.as_ref().and_then(|l| l.path.parent().map(Path::to_path_buf)))
            .unwrap_or_else(crate::picker::home_dir)
    }

    pub fn archive_stem(&self) -> String {
        let Some(l) = &self.listing else { return String::new() };
        let name = l.path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let lower = name.to_lowercase();
        for ext in [".tar.gz", ".tar.bz2", ".tar.xz", ".tgz", ".tbz2", ".txz", ".zip", ".7z", ".tar", ".gz", ".bz2", ".xz"] {
            if lower.ends_with(ext) && name.len() > ext.len() {
                return name[..name.len() - ext.len()].to_string();
            }
        }
        name
    }

    /// Builds the extract job from the dialog's choices (same path as pressing 압축 풀기).
    pub fn extract_job_from(&self, dlg: &ExtractDlg) -> Option<JobKind> {
        let listing = self.listing.clone()?;
        let mut dest = PathBuf::from(dlg.dest.trim());
        if dlg.own_folder {
            dest = dest.join(self.archive_stem());
        }
        let selection = if dlg.selected_only && !self.selected.is_empty() { Some(self.selection_vec()) } else { None };
        Some(JobKind::Extract {
            listing,
            opts: ExtractOptions {
                dest,
                strip_prefix: if selection.is_some() { self.cur_dir.clone() } else { String::new() },
                selection,
                password: self.known_password.clone(),
                overwrite: dlg.overwrite,
            },
            then: if dlg.open_after { AfterExtract::OpenFolder } else { AfterExtract::Nothing },
        })
    }

    pub fn new_extract_dialog(&self) -> ExtractDlg {
        ExtractDlg {
            dest: self.default_extract_dest().display().to_string(),
            selected_only: !self.selected.is_empty(),
            can_selected: !self.selected.is_empty(),
            own_folder: true,
            overwrite: Overwrite::Rename,
            open_after: false,
        }
    }

    pub fn new_create_dialog(sources: Vec<PathBuf>) -> CreateDlg {
        let dest = match sources.first() {
            Some(first) => {
                let parent = first.parent().map(Path::to_path_buf).unwrap_or_default();
                let base = if sources.len() == 1 {
                    first.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| tr!("새 압축 파일", "New archive").into())
                } else {
                    parent.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| tr!("새 압축 파일", "New archive").into())
                };
                parent.join(format!("{base}.zip")).display().to_string()
            }
            None => crate::picker::home_dir().join(format!("{}.zip", tr!("새 압축 파일", "New archive"))).display().to_string(),
        };
        CreateDlg { sources, dest, level: 6, use_password: false, password: String::new(), password2: String::new(), show_password: false, error: None }
    }

    /// Validates the create dialog and returns the job (same path as pressing 압축 시작).
    pub fn create_job_from(dlg: &mut CreateDlg) -> Option<JobKind> {
        if dlg.sources.is_empty() {
            dlg.error = Some(tr!("압축할 파일이나 폴더를 추가하세요.", "Add files or folders to compress.").into());
            return None;
        }
        let mut dest = PathBuf::from(dlg.dest.trim());
        if dest.as_os_str().is_empty() {
            dlg.error = Some(tr!("저장할 압축 파일 이름을 입력하세요.", "Enter a name for the new archive.").into());
            return None;
        }
        if dest.extension().is_none() {
            dest.set_extension("zip");
        }
        if dlg.use_password {
            if dlg.password.is_empty() {
                dlg.error = Some(tr!("암호를 입력하세요.", "Enter a password.").into());
                return None;
            }
            if dlg.password != dlg.password2 {
                dlg.error = Some(tr!("두 암호가 서로 다릅니다.", "The two passwords do not match.").into());
                return None;
            }
        }
        Some(JobKind::Create {
            dest,
            sources: dlg.sources.clone(),
            opts: ZipOptions { level: dlg.level, password: dlg.use_password.then(|| dlg.password.clone()) },
        })
    }

    pub fn poll_job(&mut self) {
        let Some(job) = &self.job else { return };
        let res = match job.rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err(anyhow::anyhow!(tr!("작업이 비정상적으로 끝났습니다.", "The operation stopped unexpectedly."))),
        };
        let job = self.job.take().expect("job");
        let elapsed = job.started.elapsed().as_secs_f32();
        match res {
            Ok(JobOutcome::Opened(l)) => {
                if let JobKind::Open { password, .. } = &job.kind {
                    self.known_password = password.clone();
                }
                self.set_listing(l);
            }
            Ok(JobOutcome::Extracted(rep, then, dest)) => {
                if let JobKind::Extract { opts, .. } = &job.kind {
                    if opts.password.is_some() {
                        self.known_password = opts.password.clone();
                    }
                }
                match then {
                    AfterExtract::OpenFile(p) => {
                        let target = rep.written.first().cloned().unwrap_or(p);
                        self.status = trf!("파일을 열었습니다: {}", "Opened: {}", target.display());
                        if !self.quiet {
                            let _ = open::that_detached(&target);
                        }
                    }
                    other => {
                        let (files, size, place) = (rep.files, engine::format_size(rep.bytes), dest.display());
                        let mut text = trf!(
                            "파일 {files}개 ({size})를 풀었습니다.\n위치: {place}\n걸린 시간: {elapsed:.1}초",
                            "Extracted {files} file(s) ({size}).\nLocation: {place}\nTime: {elapsed:.1} s"
                        );
                        let (existing, unsafe_paths) = (rep.skipped_existing, rep.skipped_unsafe);
                        if existing > 0 {
                            text.push_str(&trf!("\n이미 있는 파일 {existing}개는 건너뛰었습니다.", "\nSkipped {existing} file(s) that already existed."));
                        }
                        if unsafe_paths > 0 {
                            text.push_str(&trf!("\n안전하지 않은 경로 {unsafe_paths}개는 풀지 않았습니다.", "\nDid not extract {unsafe_paths} item(s) with unsafe paths."));
                        }
                        self.status = trf!("압축 풀기 완료 — 파일 {files}개", "Extraction complete — {files} file(s)");
                        if other == AfterExtract::OpenFolder && !self.quiet {
                            let _ = open::that_detached(&dest);
                        }
                        if !self.quiet {
                            self.dialog = Some(Dialog::Message { title: tr!("압축 풀기 완료", "Extraction complete").into(), text, error: false });
                        }
                    }
                }
                if let Some(st) = &mut self.selftest {
                    st.extracted = Some(rep);
                }
            }
            Ok(JobOutcome::Created(l, n)) => {
                let name = l.path.display().to_string();
                self.known_password = None;
                if let JobKind::Create { opts, .. } = &job.kind {
                    self.known_password = opts.password.clone();
                }
                self.set_listing(l);
                self.status = trf!(
                    "압축 완료 — 파일 {n}개를 {name}에 담았습니다. ({elapsed:.1}초)",
                    "Compression complete — {n} file(s) saved to {name} ({elapsed:.1} s)"
                );
            }
            Ok(JobOutcome::Modified(l, msg)) => {
                self.set_listing(l);
                self.status = msg;
            }
            Err(e) => match archive_error(&e) {
                Some(ArchiveError::PasswordRequired) | Some(ArchiveError::WrongPassword) => {
                    let wrong = archive_error(&e) == Some(&ArchiveError::WrongPassword);
                    self.status = e.to_string();
                    self.dialog = Some(Dialog::Password { job: job.kind, text: String::new(), wrong });
                }
                Some(ArchiveError::Cancelled) => self.status = ArchiveError::Cancelled.to_string(),
                _ => {
                    let text = crate::i18n::describe_error(&e);
                    let first = text.lines().next().unwrap_or_default();
                    self.status = trf!("오류: {first}", "Error: {first}");
                    if let Some(st) = &mut self.selftest {
                        st.errors.push(format!("{e:#}"));
                    }
                    self.dialog = Some(Dialog::Message { title: tr!("오류", "Error").into(), text, error: true });
                }
            },
        }
    }

    pub fn process_actions(&mut self, ctx: &egui::Context) {
        let actions = std::mem::take(&mut self.actions);
        for a in actions {
            self.apply(ctx, a);
        }
    }

    pub fn apply(&mut self, ctx: &egui::Context, a: Action) {
        match a {
            Action::PickOpen => {
                let start = self.listing.as_ref().and_then(|l| l.path.parent().map(Path::to_path_buf));
                self.picker = Some(Picker::new(PickPurpose::OpenArchive, start, String::new()));
            }
            Action::OpenPath(p) => {
                self.known_password = None;
                self.start_job(JobKind::Open { path: p, encoding: NameEncoding::Auto, password: None });
            }
            Action::Close => {
                self.listing = None;
                self.view.clear();
                self.selected.clear();
                self.cur_dir.clear();
                self.status = self.idle_status();
            }
            Action::NewArchive(v) => {
                if v.is_empty() {
                    self.picker = Some(Picker::new(PickPurpose::CreateSources, None, String::new()));
                } else {
                    self.dialog = Some(Dialog::Create(Self::new_create_dialog(v)));
                }
            }
            Action::ShowExtract => {
                if self.listing.is_some() {
                    self.dialog = Some(Dialog::Extract(self.new_extract_dialog()));
                }
            }
            Action::ShowAdd(v) => {
                if self.editable() {
                    let empty = v.is_empty();
                    self.dialog = Some(Dialog::Add(AddDlg { sources: v, level: 6, use_password: false, password: String::new(), show_password: false }));
                    if empty {
                        self.picker = Some(Picker::new(PickPurpose::AddSources, None, String::new()));
                    }
                }
            }
            Action::AskDelete => {
                if self.editable() && !self.selected.is_empty() {
                    self.dialog = Some(Dialog::ConfirmDelete(self.selection_vec()));
                }
            }
            Action::Navigate(d) => self.navigate(d),
            Action::Up => {
                let from = self.cur_dir.clone();
                self.navigate(engine::parent_dir(&from));
                self.selected.insert(from);
            }
            Action::OpenEntry(path) => {
                let Some(l) = self.listing.clone() else { return };
                let dir = std::env::temp_dir().join(format!("rustzip-view-{}", std::process::id()));
                let rel = path.rsplit('/').next().unwrap_or(&path).to_string();
                let job = JobKind::Extract {
                    listing: l,
                    opts: ExtractOptions {
                        dest: dir.clone(),
                        selection: Some(vec![path.clone()]),
                        strip_prefix: engine::parent_dir(&path),
                        password: self.known_password.clone(),
                        overwrite: Overwrite::Always,
                    },
                    then: AfterExtract::OpenFile(dir.join(rel)),
                };
                self.start_job(job);
            }
            Action::SetEncoding(enc) => {
                if let Some(l) = self.listing.clone() {
                    self.start_job(JobKind::Open { path: l.path.clone(), encoding: enc, password: self.known_password.clone() });
                } else {
                    self.encoding = enc;
                }
            }
            Action::SetLanguage(lang) => self.set_language(lang),
            Action::Start(job) => self.start_job(job),
            Action::Dropped(paths) => {
                if paths.len() == 1 && engine::is_archive_file(&paths[0]) {
                    self.apply(ctx, Action::OpenPath(paths[0].clone()));
                } else if self.editable() {
                    self.dialog = Some(Dialog::DropChoice(paths));
                } else {
                    self.dialog = Some(Dialog::Create(Self::new_create_dialog(paths)));
                }
            }
            Action::About => self.dialog = Some(Dialog::About),
            Action::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }
}

impl eframe::App for RustZipApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frame_ui(ui);
        let ctx = ui.ctx().clone();
        crate::selftest::drive(self, &ctx);
        crate::shot::drive(self, &ctx);
    }
}
