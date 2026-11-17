//! RustZip archive engine: listing, extracting, creating and editing archives.
//!
//! Everything here is GUI-free so it can be unit tested and run on a worker thread.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Context as _, Result};
use chrono::{DateTime, Local, NaiveDateTime, TimeZone};

mod codec;
mod create;
mod filter;
mod formats;
mod lzw;
mod walk;

pub use filter::Filter;

// ---------------------------------------------------------------------------
// Errors that the GUI reacts to specially
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveError {
    /// The archive (or a selected entry) is encrypted and no password was given.
    PasswordRequired,
    /// The password given does not match.
    WrongPassword,
    /// The user pressed cancel.
    Cancelled,
    /// The operation is not possible for this archive type.
    Unsupported(String),
}

impl fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArchiveError::PasswordRequired => f.write_str(tr!("암호가 필요합니다.", "A password is required.")),
            ArchiveError::WrongPassword => f.write_str(tr!("암호가 올바르지 않습니다.", "The password is incorrect.")),
            ArchiveError::Cancelled => f.write_str(tr!("작업을 취소했습니다.", "The operation was cancelled.")),
            ArchiveError::Unsupported(s) => f.write_str(&trf!("지원하지 않는 작업입니다: {s}", "Not supported: {s}")),
        }
    }
}

impl std::error::Error for ArchiveError {}

/// Returns the special error inside an anyhow error, if any.
pub fn archive_error(err: &anyhow::Error) -> Option<&ArchiveError> {
    err.downcast_ref::<ArchiveError>()
}

/// Adds a leading explanation to a library error; password and cancel errors
/// already say everything and are passed through unchanged.
fn with_note(err: anyhow::Error, note: impl FnOnce() -> String) -> anyhow::Error {
    if archive_error(&err).is_some() {
        err
    } else {
        err.context(note())
    }
}

/// Names the archive entry that failed to extract.
fn entry_error(err: anyhow::Error, path: &str) -> anyhow::Error {
    with_note(err, || trf!("'{path}' 항목을 풀 수 없습니다.", "Could not extract '{path}'."))
}

// ---------------------------------------------------------------------------
// Progress shared between the worker and the GUI
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct Progress {
    pub total: AtomicU64,
    pub done: AtomicU64,
    pub cancel: AtomicBool,
    pub current: Mutex<String>,
}

impl Progress {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    pub fn set_total(&self, total: u64) {
        self.total.store(total, Ordering::Relaxed);
    }
    pub fn add(&self, n: u64) {
        self.done.fetch_add(n, Ordering::Relaxed);
    }
    pub fn set_current(&self, s: &str) {
        if let Ok(mut c) = self.current.lock() {
            c.clear();
            c.push_str(s);
        }
    }
    pub fn current(&self) -> String {
        self.current.lock().map(|c| c.clone()).unwrap_or_default()
    }
    pub fn fraction(&self) -> f32 {
        let t = self.total.load(Ordering::Relaxed);
        let d = self.done.load(Ordering::Relaxed);
        if t == 0 {
            0.0
        } else {
            (d as f64 / t as f64).clamp(0.0, 1.0) as f32
        }
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    fn check(&self) -> Result<()> {
        if self.cancelled() {
            Err(ArchiveError::Cancelled.into())
        } else {
            Ok(())
        }
    }
}

/// Copies `r` into `w` in chunks, reporting progress and honouring cancel.
fn copy_with_progress(r: &mut dyn Read, w: &mut dyn Write, p: &Progress) -> Result<u64> {
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        p.check()?;
        let n = match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        w.write_all(&buf[..n])?;
        total += n as u64;
        p.add(n as u64);
    }
    Ok(total)
}

// ---------------------------------------------------------------------------
// Formats and name encodings
// ---------------------------------------------------------------------------

/// The archive structure, independent of any outer compression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Container {
    Zip,
    SevenZ,
    Rar,
    Tar,
    Cpio,
    Ar,
    Lzh,
    Arj,
    Cab,
    Iso,
    Alz,
    Egg,
    /// A single compressed file (`a.txt.gz`); the filter says how.
    Raw,
}

impl Container {
    pub fn label(self) -> &'static str {
        match self {
            Container::Zip => "ZIP",
            Container::SevenZ => "7Z",
            Container::Rar => "RAR",
            Container::Tar => "TAR",
            Container::Cpio => "CPIO",
            Container::Ar => "AR",
            Container::Lzh => "LZH",
            Container::Arj => "ARJ",
            Container::Cab => "CAB",
            Container::Iso => "ISO",
            Container::Alz => "ALZ",
            Container::Egg => "EGG",
            Container::Raw => "",
        }
    }

    /// The entry-by-entry reader, for formats that have one.
    fn walker(self) -> Option<walk::Walker> {
        Some(match self {
            Container::Tar => formats::tar::walk,
            Container::Cpio => formats::cpio::walk,
            Container::Ar => formats::ar::walk,
            Container::Lzh => formats::lzh::walk,
            Container::Arj => formats::arj::walk,
            Container::Cab => formats::cab::walk,
            Container::Iso => formats::iso::walk,
            Container::Alz => formats::alz::walk,
            Container::Egg => formats::egg::walk,
            Container::Zip | Container::SevenZ | Container::Rar | Container::Raw => return None,
        })
    }
}

/// What a file is: a container, optionally inside a compression filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArchiveKind {
    pub container: Container,
    pub filter: Filter,
}

impl ArchiveKind {
    pub const ZIP: ArchiveKind = ArchiveKind::plain(Container::Zip);
    pub const SEVENZ: ArchiveKind = ArchiveKind::plain(Container::SevenZ);
    pub const TAR: ArchiveKind = ArchiveKind::plain(Container::Tar);

    pub const fn plain(container: Container) -> Self {
        Self { container, filter: Filter::None }
    }
    pub const fn new(container: Container, filter: Filter) -> Self {
        Self { container, filter }
    }

    pub fn label(self) -> String {
        match (self.container, self.filter) {
            (Container::Raw, f) => f.label().to_string(),
            (c, Filter::None) => c.label().to_string(),
            (c, f) => format!("{}.{}", c.label(), f.label()),
        }
    }

    /// The usual file name ending, without the dot (`tar.gz`, `zip`, `zst`).
    pub fn extension(self) -> String {
        let base = match self.container {
            Container::Raw => return self.filter.ext().to_string(),
            Container::SevenZ => "7z".to_string(),
            c => c.label().to_lowercase(),
        };
        match self.filter {
            Filter::None => base,
            f => format!("{base}.{}", f.ext()),
        }
    }

    pub fn is_editable(self) -> bool {
        self == ArchiveKind::ZIP
    }

    /// Every kind RustZip can create, in menu order.
    pub fn creatable() -> Vec<ArchiveKind> {
        let filters = Filter::ALL.into_iter().filter(|f| f.can_write());
        [ArchiveKind::ZIP, ArchiveKind::SEVENZ, ArchiveKind::TAR]
            .into_iter()
            .chain(filters.clone().map(|f| ArchiveKind::new(Container::Tar, f)))
            .chain(filters.map(|f| ArchiveKind::new(Container::Raw, f)))
            .collect()
    }

    /// Algorithms to choose from inside the archive (empty: the filter decides).
    pub fn methods(self) -> &'static [Method] {
        match self.container {
            Container::Zip => &[Method::Deflate, Method::Bzip2, Method::Xz, Method::Zstd, Method::Ppmd, Method::Store],
            Container::SevenZ => &[Method::Lzma2, Method::Lzma, Method::Ppmd, Method::Bzip2, Method::Deflate, Method::Zstd, Method::Brotli, Method::Lz4, Method::Store],
            _ => &[],
        }
    }

    /// [`Self::methods`] minus the ones that cannot be combined with a password.
    /// sevenz-rust2's Brotli decoder reads past the stream into the AES padding.
    pub fn methods_for(self, password: bool) -> Vec<Method> {
        self.methods().iter().copied().filter(|&m| !(password && self.container == Container::SevenZ && m == Method::Brotli)).collect()
    }

    pub fn supports_password(self) -> bool {
        matches!(self.container, Container::Zip | Container::SevenZ) && self.filter == Filter::None
    }

    /// Single compressed files (`.gz`, `.zst`) hold exactly one file.
    pub fn single_file_only(self) -> bool {
        self.container == Container::Raw
    }
}

/// How to decode ZIP (and TAR) file names that are not marked as UTF-8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameEncoding {
    /// UTF-8 when flagged or valid, otherwise Korean CP949, otherwise CP437.
    Auto,
    Utf8,
    Cp949,
    Cp437,
}

impl NameEncoding {
    pub const ALL: [NameEncoding; 4] = [
        NameEncoding::Auto,
        NameEncoding::Utf8,
        NameEncoding::Cp949,
        NameEncoding::Cp437,
    ];
    pub fn label(self) -> &'static str {
        match self {
            NameEncoding::Auto => tr!("자동 감지", "Auto-detect"),
            NameEncoding::Utf8 => tr!("유니코드 (UTF-8)", "Unicode (UTF-8)"),
            NameEncoding::Cp949 => tr!("한국어 (CP949/EUC-KR)", "Korean (CP949/EUC-KR)"),
            NameEncoding::Cp437 => tr!("서유럽 (CP437)", "Western (CP437)"),
        }
    }
}

fn decode_cp437(raw: &[u8]) -> String {
    // Upper half of code page 437.
    const HIGH: [char; 128] = [
        'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', 'É', 'æ',
        'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ', 'á', 'í', 'ó', 'ú',
        'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»', '░', '▒', '▓', '│', '┤', '╡',
        '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐', '└', '┴', '┬', '├', '─', '┼', '╞', '╟',
        '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧', '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘',
        '┌', '█', '▄', '▌', '▐', '▀', 'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ',
        '∞', 'φ', 'ε', '∩', '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²',
        '■', '\u{a0}',
    ];
    raw.iter()
        .map(|&b| if b < 0x80 { b as char } else { HIGH[(b - 0x80) as usize] })
        .collect()
}

fn decode_cp949(raw: &[u8]) -> (String, bool) {
    // encoding_rs's EUC_KR is the WHATWG "euc-kr" = Windows code page 949 (a superset of EUC-KR).
    let (s, _, had_errors) = encoding_rs::EUC_KR.decode(raw);
    (s.into_owned(), had_errors)
}

/// Decodes a raw archive file name. `utf8_flag` is the ZIP "language encoding" bit.
pub fn decode_name(raw: &[u8], utf8_flag: bool, enc: NameEncoding) -> String {
    match enc {
        NameEncoding::Utf8 => String::from_utf8_lossy(raw).into_owned(),
        NameEncoding::Cp949 => decode_cp949(raw).0,
        NameEncoding::Cp437 => decode_cp437(raw),
        NameEncoding::Auto => {
            if utf8_flag {
                return String::from_utf8_lossy(raw).into_owned();
            }
            if let Ok(s) = std::str::from_utf8(raw) {
                return s.to_string();
            }
            let (s, had_errors) = decode_cp949(raw);
            if !had_errors {
                return s;
            }
            decode_cp437(raw)
        }
    }
}

/// Turns any separator style into a clean `a/b/c` path (no leading/trailing slash, no `.`).
pub fn normalize_entry_path(name: &str) -> String {
    name.split(['/', '\\'])
        .filter(|c| !c.is_empty() && *c != ".")
        .collect::<Vec<_>>()
        .join("/")
}

/// Converts an archive entry path into a safe relative path, or `None` if it
/// would escape the destination folder (absolute paths, `..`, drive letters).
pub fn safe_relative_path(name: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for comp in name.split(['/', '\\']) {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp == ".." || comp.contains(':') || comp.contains('\0') {
            return None;
        }
        out.push(comp);
    }
    // Paranoia: make sure the result is purely normal components.
    if out.components().all(|c| matches!(c, Component::Normal(_))) && !out.as_os_str().is_empty() {
        Some(out)
    } else {
        None
    }
}

const SEVENZ_MAGIC: [u8; 6] = [b'7', b'z', 0xBC, 0xAF, 0x27, 0x1C];
const RAR_MAGIC: &[u8] = b"Rar!\x1a\x07";

fn looks_like_tar(header: &[u8]) -> bool {
    if header.len() < 512 {
        return false;
    }
    if &header[257..262] == b"ustar" {
        return true;
    }
    // Old V7 tar: verify the header checksum.
    let stored = std::str::from_utf8(&header[148..156])
        .ok()
        .and_then(|s| u32::from_str_radix(s.trim_matches(|c: char| c == '\0' || c == ' '), 8).ok());
    let Some(stored) = stored else { return false };
    let sum: u32 = header
        .iter()
        .enumerate()
        .map(|(i, &b)| if (148..156).contains(&i) { 32 } else { b as u32 })
        .sum();
    sum == stored && header[0] != 0
}

fn read_up_to(r: &mut dyn Read, n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    let mut got = 0;
    while got < n {
        match r.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(k) => got += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    buf.truncate(got);
    buf
}

fn looks_like_cpio(h: &[u8]) -> bool {
    h.starts_with(b"070707") || h.starts_with(b"070701") || h.starts_with(b"070702") || h.starts_with(&[0xC7, 0x71]) || h.starts_with(&[0x71, 0xC7])
}

fn looks_like_lzh(h: &[u8]) -> bool {
    h.len() > 21 && h[2] == b'-' && h[6] == b'-' && matches!(&h[3..5], b"lh" | b"lz" | b"pm")
}

/// Containers that can be told apart by their first bytes.
fn sniff_container(h: &[u8]) -> Option<Container> {
    let c = if h.starts_with(b"PK\x03\x04") || h.starts_with(b"PK\x05\x06") || h.starts_with(b"PK\x07\x08") {
        Container::Zip
    } else if h.starts_with(&SEVENZ_MAGIC) {
        Container::SevenZ
    } else if h.starts_with(RAR_MAGIC) {
        Container::Rar
    } else if h.starts_with(b"ALZ\x01") {
        Container::Alz
    } else if h.starts_with(b"EGGA") {
        Container::Egg
    } else if h.starts_with(b"MSCF") {
        Container::Cab
    } else if h.starts_with(&[0x60, 0xEA]) {
        Container::Arj
    } else if h.starts_with(b"!<arch>\n") {
        Container::Ar
    } else if looks_like_cpio(h) {
        Container::Cpio
    } else if looks_like_lzh(h) {
        Container::Lzh
    } else if looks_like_tar(h) {
        Container::Tar
    } else {
        return None;
    };
    Some(c)
}

fn is_iso(f: &mut File) -> bool {
    let mut b = [0u8; 5];
    f.seek(SeekFrom::Start(0x8001)).is_ok() && f.read_exact(&mut b).is_ok() && &b == b"CD001"
}

/// Self-extracting `.exe`: a ZIP or RAR appended to a Windows program.
fn sniff_sfx(path: &Path) -> Option<Container> {
    if zip::ZipArchive::new(BufReader::new(File::open(path).ok()?)).is_ok() {
        return Some(Container::Zip);
    }
    let head = read_up_to(&mut File::open(path).ok()?, 4 << 20);
    head.windows(RAR_MAGIC.len()).any(|w| w == RAR_MAGIC).then_some(Container::Rar)
}

fn lower_ext(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// Works out the archive type from the file's first bytes (falling back to the extension).
pub fn detect_kind(path: &Path) -> Result<ArchiveKind> {
    let mut f = File::open(path).with_context(|| trf!("파일을 열 수 없습니다: {}", "Cannot open the file: {}", path.display()))?;
    let head = read_up_to(&mut f, 512);
    let ext = lower_ext(path);
    if let Some(c) = sniff_container(&head) {
        return Ok(ArchiveKind::plain(c));
    }
    if let Some(filter) = Filter::sniff(&head, &ext) {
        let inner = filter.reader(File::open(path)?).map(|mut r| read_up_to(&mut r, 512)).unwrap_or_default();
        let container = match sniff_container(&inner) {
            Some(c @ (Container::Tar | Container::Cpio | Container::Ar)) => c,
            _ => Container::Raw,
        };
        return Ok(ArchiveKind::new(container, filter));
    }
    if is_iso(&mut f) {
        return Ok(ArchiveKind::plain(Container::Iso));
    }
    if head.starts_with(b"MZ") {
        if let Some(c) = sniff_sfx(path) {
            return Ok(ArchiveKind::plain(c));
        }
    }
    match ext.as_str() {
        "zip" => Ok(ArchiveKind::ZIP),
        "tar" => Ok(ArchiveKind::TAR),
        _ => bail!(tr!("지원하지 않는 압축 형식이거나 손상된 파일입니다.", "Unsupported archive format, or the file is damaged.")),
    }
}

/// True when the path looks like an archive RustZip can open (cheap, for drag and drop).
pub fn is_archive_file(path: &Path) -> bool {
    path.is_file() && detect_kind(path).is_ok()
}

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Entry {
    /// Index inside the archive (ZIP index, 7z file index, tar order).
    pub index: usize,
    /// Normalized path `folder/sub/name`.
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    /// Compressed size, when the format stores it per entry.
    pub packed: Option<u64>,
    pub modified: Option<NaiveDateTime>,
    pub encrypted: bool,
    pub method: String,
}

impl Entry {
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

#[derive(Debug, Clone)]
pub struct Listing {
    pub path: PathBuf,
    pub kind: ArchiveKind,
    pub entries: Vec<Entry>,
    pub encoding: NameEncoding,
    pub archive_size: u64,
}

impl Listing {
    pub fn any_encrypted(&self) -> bool {
        self.entries.iter().any(|e| e.encrypted)
    }
    pub fn total_size(&self) -> u64 {
        self.entries.iter().filter(|e| !e.is_dir).map(|e| e.size).sum()
    }
    pub fn file_count(&self) -> usize {
        self.entries.iter().filter(|e| !e.is_dir).count()
    }
}

fn zip_datetime(dt: Option<zip::DateTime>) -> Option<NaiveDateTime> {
    dt.and_then(|d| NaiveDateTime::try_from(d).ok())
}

fn unix_to_naive(secs: i64) -> Option<NaiveDateTime> {
    if secs <= 0 {
        return None;
    }
    Local.timestamp_opt(secs, 0).single().map(|d| d.naive_local())
}

/// MS-DOS packed date and time (used by RAR, ALZ, ARJ...).
pub(crate) fn dos_to_naive(t: u32) -> Option<NaiveDateTime> {
    let (date, time) = (t >> 16, t & 0xFFFF);
    chrono::NaiveDate::from_ymd_opt(1980 + (date >> 9) as i32, (date >> 5) & 0xF, date & 0x1F)
        .and_then(|d| d.and_hms_opt(time >> 11, (time >> 5) & 0x3F, (time & 0x1F) * 2))
}

fn nt_to_naive(t: sevenz_rust2::NtTime) -> Option<NaiveDateTime> {
    let ticks: u64 = t.into();
    if ticks == 0 {
        return None;
    }
    let secs = (ticks / 10_000_000) as i64 - 11_644_473_600;
    unix_to_naive(secs)
}

fn zip_method_label(m: zip::CompressionMethod) -> String {
    match m {
        zip::CompressionMethod::Stored => tr!("저장", "Store").into(),
        zip::CompressionMethod::Deflated => "Deflate".into(),
        other => format!("{other:?}"),
    }
}

pub fn list(path: &Path, encoding: NameEncoding, progress: &Progress) -> Result<Listing> {
    list_with_password(path, encoding, None, progress)
}

/// Like [`list`], with a password for 7z archives whose file list itself is encrypted.
pub fn list_with_password(path: &Path, encoding: NameEncoding, password: Option<&str>, progress: &Progress) -> Result<Listing> {
    let kind = detect_kind(path)?;
    let archive_size = fs::metadata(path)?.len();
    let entries = match kind.container {
        Container::Zip => list_zip(path, encoding),
        Container::SevenZ => list_7z(path, password),
        Container::Rar => formats::rar::list(path, password),
        Container::Raw => list_single(path, kind.filter, progress),
        c => {
            let walker = c.walker().expect("walker for entry-by-entry formats");
            walk::list(walker, &walk::Source { path, filter: kind.filter, encoding, password, input: progress })
        }
    }
    .map_err(|e| with_note(e, || trf!("{} 파일의 목록을 읽을 수 없습니다.", "Cannot read the contents of the {} file.", kind.label())))?;
    Ok(Listing { path: path.to_path_buf(), kind, entries, encoding, archive_size })
}

fn list_zip(path: &Path, encoding: NameEncoding) -> Result<Vec<Entry>> {
    let archive = zip::ZipArchive::new(BufReader::new(File::open(path)?))?;
    let mut out = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let e = archive.by_index_data(i)?;
        // Bit 11 = "language encoding" (EFS): the name is UTF-8.
        let utf8 = e.flags().as_u16() & 0x0800 != 0;
        let name = decode_name(e.name_raw(), utf8, encoding);
        let is_dir = name.ends_with('/') || name.ends_with('\\') || e.is_dir();
        let path = normalize_entry_path(&name);
        if path.is_empty() {
            continue;
        }
        out.push(Entry {
            index: i,
            path,
            is_dir,
            size: e.size(),
            packed: Some(e.compressed_size()),
            modified: zip_datetime(e.last_modified()),
            encrypted: e.encrypted(),
            method: zip_method_label(e.compression()),
        });
    }
    Ok(out)
}

fn sevenz_err(e: sevenz_rust2::Error) -> anyhow::Error {
    match e {
        sevenz_rust2::Error::PasswordRequired => ArchiveError::PasswordRequired.into(),
        sevenz_rust2::Error::MaybeBadPassword(_) => ArchiveError::WrongPassword.into(),
        other => anyhow::Error::new(other),
    }
}

fn list_7z(path: &Path, password: Option<&str>) -> Result<Vec<Entry>> {
    // Archives with encrypted headers need the password even to list; the GUI asks then.
    let archive = match password.filter(|p| !p.is_empty()) {
        Some(pw) => sevenz_rust2::Archive::open_with_password(path, &sevenz_rust2::Password::new(pw)),
        None => sevenz_rust2::Archive::open(path),
    }
    .map_err(sevenz_err)?;
    let mut out = Vec::new();
    for (i, f) in archive.files.iter().enumerate() {
        let p = normalize_entry_path(&f.name);
        if p.is_empty() || f.is_anti_item {
            continue;
        }
        let coders: Vec<&'static str> = archive
            .stream_map
            .file_block_index
            .get(i)
            .copied()
            .flatten()
            .and_then(|b| archive.blocks.get(b))
            .map(|b| b.coders.iter().filter_map(|c| sevenz_rust2::EncoderMethod::by_id(c.encoder_method_id())).map(|m| m.name()).collect())
            .unwrap_or_default();
        let encrypted = coders.contains(&sevenz_rust2::EncoderMethod::AES256_SHA256.name());
        let method = coders.iter().filter(|&&n| n != sevenz_rust2::EncoderMethod::AES256_SHA256.name()).copied().collect::<Vec<_>>().join("+");
        out.push(Entry {
            index: i,
            path: p,
            is_dir: f.is_directory,
            size: f.size,
            packed: None,
            modified: if f.has_last_modified_date { nt_to_naive(f.last_modified_date) } else { None },
            encrypted,
            method: if method.is_empty() { "7z".into() } else { method },
        });
    }
    // Per-entry packed sizes are not stored in 7z (blocks are shared); spread the
    // block's packed size proportionally so the column still means something.
    let total: u64 = out.iter().map(|e| e.size).sum();
    let packed_total: u64 = archive.pack_sizes().iter().sum();
    if total > 0 {
        for e in out.iter_mut().filter(|e| !e.is_dir) {
            e.packed = Some(((e.size as u128 * packed_total as u128) / total as u128) as u64);
        }
    }
    Ok(out)
}

/// Name of the file inside a single-file archive: `a.txt.gz` holds `a.txt`.
fn single_inner_name(path: &Path, filter: Filter) -> String {
    let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let lower = name.to_lowercase();
    let ext = format!(".{}", filter.ext().to_lowercase());
    if lower.ends_with(&ext) && lower.len() > ext.len() {
        return name[..name.len() - ext.len()].to_string();
    }
    for short in filter.tar_shorthands() {
        if let Some(stem) = lower.strip_suffix(&format!(".{short}")) {
            return format!("{}.tar", &name[..stem.len()]);
        }
    }
    format!("{name}.out")
}

fn list_single(path: &Path, filter: Filter, progress: &Progress) -> Result<Vec<Entry>> {
    let packed = fs::metadata(path)?.len();
    let mut name = single_inner_name(path, filter);
    let mut modified = fs::metadata(path)?.modified().ok().map(|t| DateTime::<Local>::from(t).naive_local());
    if filter == Filter::Gzip {
        if let Some(h) = flate2::read::GzDecoder::new(File::open(path)?).header() {
            if let Some(n) = h.filename() {
                let n = normalize_entry_path(&decode_name(n, false, NameEncoding::Auto));
                if let Some(last) = n.rsplit('/').next().filter(|l| !l.is_empty()) {
                    name = last.to_string();
                }
            }
            if h.mtime() != 0 {
                modified = unix_to_naive(h.mtime() as i64);
            }
        }
    }
    // Only decompressing tells the real size; count compressed bytes for the progress bar.
    let mut d = filter.reader(walk::CountingReader { inner: File::open(path)?, progress })?;
    progress.set_total(packed);
    let size = io::copy(&mut d, &mut io::sink()).map_err(|e| walk::cancel_aware(e, progress))?;
    Ok(vec![Entry {
        index: 0,
        path: name,
        is_dir: false,
        size,
        packed: Some(packed),
        modified,
        encrypted: false,
        method: filter.algorithm().into(),
    }])
}

// ---------------------------------------------------------------------------
// Folder tree navigation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Child {
    pub name: String,
    /// Full path inside the archive.
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub packed: Option<u64>,
    pub modified: Option<NaiveDateTime>,
    pub encrypted: bool,
    /// Compression method of a file; empty for folders.
    pub method: String,
    /// Number of files below a folder.
    pub file_count: usize,
}

/// Lists what is directly inside `dir` ("" = archive root), including folders that
/// only exist implicitly through their files.
pub fn children(entries: &[Entry], dir: &str) -> Vec<Child> {
    let prefix = if dir.is_empty() { String::new() } else { format!("{dir}/") };
    let mut map: BTreeMap<(bool, String), Child> = BTreeMap::new();
    for e in entries {
        let Some(rest) = e.path.strip_prefix(&prefix) else { continue };
        if rest.is_empty() {
            continue;
        }
        match rest.split_once('/') {
            Some((first, _)) => {
                let c = map.entry((false, first.to_string())).or_insert_with(|| Child {
                    name: first.to_string(),
                    path: format!("{prefix}{first}"),
                    is_dir: true,
                    size: 0,
                    packed: None,
                    modified: None,
                    encrypted: false,
                    method: String::new(),
                    file_count: 0,
                });
                if !e.is_dir {
                    c.size += e.size;
                    c.file_count += 1;
                    if let Some(p) = e.packed {
                        c.packed = Some(c.packed.unwrap_or(0) + p);
                    }
                }
            }
            None if e.is_dir => {
                let c = map.entry((false, rest.to_string())).or_insert_with(|| Child {
                    name: rest.to_string(),
                    path: e.path.clone(),
                    is_dir: true,
                    size: 0,
                    packed: None,
                    modified: None,
                    encrypted: false,
                    method: String::new(),
                    file_count: 0,
                });
                c.modified = e.modified;
            }
            None => {
                map.insert(
                    (true, rest.to_string()),
                    Child {
                        name: rest.to_string(),
                        path: e.path.clone(),
                        is_dir: false,
                        size: e.size,
                        packed: e.packed,
                        modified: e.modified,
                        encrypted: e.encrypted,
                        method: e.method.clone(),
                        file_count: 1,
                    },
                );
            }
        }
    }
    map.into_values().collect()
}

/// All folder paths in the archive (explicit and implied), sorted.
pub fn all_folders(entries: &[Entry]) -> Vec<String> {
    let mut set = std::collections::BTreeSet::new();
    for e in entries {
        let parts: Vec<&str> = e.path.split('/').collect();
        let n = if e.is_dir { parts.len() } else { parts.len() - 1 };
        for k in 1..=n {
            set.insert(parts[..k].join("/"));
        }
    }
    set.into_iter().collect()
}

pub fn parent_dir(dir: &str) -> String {
    dir.rsplit_once('/').map(|(p, _)| p.to_string()).unwrap_or_default()
}

/// Is `path` one of the selected paths or inside one of the selected folders?
fn is_selected(path: &str, selection: &[String]) -> bool {
    selection.iter().any(|s| path == s || path.strip_prefix(s.as_str()).is_some_and(|r| r.starts_with('/')))
}

// ---------------------------------------------------------------------------
// Extracting
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overwrite {
    Always,
    Skip,
    Rename,
}

#[derive(Debug, Clone)]
pub struct ExtractOptions {
    pub dest: PathBuf,
    /// Entry paths (files or folders) to extract; `None` = everything.
    pub selection: Option<Vec<String>>,
    /// Folder prefix stripped from output paths (the folder being viewed).
    pub strip_prefix: String,
    pub password: Option<String>,
    pub overwrite: Overwrite,
}

#[derive(Debug, Default, Clone)]
pub struct ExtractReport {
    pub files: usize,
    pub dirs: usize,
    pub bytes: u64,
    pub skipped_existing: usize,
    pub skipped_unsafe: usize,
    pub written: Vec<PathBuf>,
}

fn unique_path(p: &Path) -> PathBuf {
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let parent = p.parent().map(Path::to_path_buf).unwrap_or_default();
    for n in 1.. {
        let cand = parent.join(format!("{stem} ({n}){ext}"));
        if !cand.exists() {
            return cand;
        }
    }
    unreachable!()
}

struct Planner<'a> {
    opts: &'a ExtractOptions,
    report: ExtractReport,
}

impl Planner<'_> {
    /// Where an entry goes, or `None` if it is not selected / unsafe / skipped.
    fn target(&mut self, entry_path: &str, is_dir: bool) -> Option<PathBuf> {
        if let Some(sel) = &self.opts.selection {
            if !is_selected(entry_path, sel) {
                return None;
            }
        }
        let rel = if self.opts.strip_prefix.is_empty() {
            entry_path
        } else {
            match entry_path.strip_prefix(&format!("{}/", self.opts.strip_prefix)) {
                Some(r) => r,
                None => entry_path,
            }
        };
        let Some(rel) = safe_relative_path(rel) else {
            self.report.skipped_unsafe += 1;
            return None;
        };
        let out = self.opts.dest.join(rel);
        if is_dir {
            return Some(out);
        }
        if out.exists() {
            match self.opts.overwrite {
                Overwrite::Always => {}
                Overwrite::Skip => {
                    self.report.skipped_existing += 1;
                    return None;
                }
                Overwrite::Rename => return Some(unique_path(&out)),
            }
        }
        Some(out)
    }

    fn make_dir(&mut self, p: &Path) -> Result<()> {
        fs::create_dir_all(p).with_context(|| trf!("폴더를 만들 수 없습니다: {}", "Cannot create the folder: {}", p.display()))?;
        self.report.dirs += 1;
        Ok(())
    }

    fn create_file(&mut self, p: &Path) -> Result<File> {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }
        File::create(p).with_context(|| trf!("파일을 만들 수 없습니다: {}", "Cannot create the file: {}", p.display()))
    }

    fn finish_file(&mut self, p: PathBuf, bytes: u64, mtime: Option<NaiveDateTime>) {
        if let Some(t) = mtime.and_then(|t| Local.from_local_datetime(&t).single()) {
            if let Ok(f) = File::options().write(true).open(&p) {
                let _ = f.set_modified(t.into());
            }
        }
        self.report.files += 1;
        self.report.bytes += bytes;
        self.report.written.push(p);
    }
}

pub fn extract(listing: &Listing, opts: &ExtractOptions, progress: &Progress) -> Result<ExtractReport> {
    fs::create_dir_all(&opts.dest)
        .with_context(|| trf!("대상 폴더를 만들 수 없습니다: {}", "Cannot create the destination folder: {}", opts.dest.display()))?;
    let selected = |e: &Entry| opts.selection.as_ref().is_none_or(|s| is_selected(&e.path, s));
    let total: u64 = listing.entries.iter().filter(|e| selected(e) && !e.is_dir).map(|e| e.size).sum();
    progress.set_total(total.max(1));
    let mut planner = Planner { opts, report: ExtractReport::default() };
    match listing.kind.container {
        Container::Zip => extract_zip(listing, &mut planner, progress)?,
        Container::SevenZ => extract_7z(listing, &mut planner, progress)?,
        Container::Rar => formats::rar::extract(listing, &mut planner, progress)?,
        Container::Raw => extract_single(listing, &mut planner, progress)?,
        c => {
            let walker = c.walker().expect("walker for entry-by-entry formats");
            let src = walk::Source {
                path: &listing.path,
                filter: listing.kind.filter,
                encoding: listing.encoding,
                password: opts.password.as_deref(),
                input: &Progress::default(),
            };
            walk::extract(walker, &src, &mut planner, progress).map_err(|e| walk::cancel_aware(e, progress))?;
        }
    }
    Ok(planner.report)
}

fn extract_zip(listing: &Listing, planner: &mut Planner<'_>, progress: &Progress) -> Result<()> {
    let mut archive = zip::ZipArchive::new(BufReader::new(File::open(&listing.path)?))?;
    let password = planner.opts.password.clone().filter(|p| !p.is_empty());
    let sel = planner.opts.selection.clone();
    let wanted: Vec<&Entry> = listing
        .entries
        .iter()
        .filter(|e| sel.as_ref().is_none_or(|s| is_selected(&e.path, s)))
        .collect();
    if password.is_none() && wanted.iter().any(|e| e.encrypted && !e.is_dir) {
        return Err(ArchiveError::PasswordRequired.into());
    }
    for e in wanted {
        progress.check()?;
        progress.set_current(&e.path);
        let Some(out) = planner.target(&e.path, e.is_dir) else {
            progress.add(if e.is_dir { 0 } else { e.size });
            continue;
        };
        if e.is_dir {
            planner.make_dir(&out)?;
            continue;
        }
        let mut zf = match (&password, e.encrypted) {
            (Some(pw), true) => archive.by_index_decrypt(e.index, pw.as_bytes()),
            _ => archive.by_index(e.index),
        }
        .map_err(|err| match err {
            zip::result::ZipError::InvalidPassword => ArchiveError::WrongPassword.into(),
            other => entry_error(other.into(), &e.path),
        })?;
        let mut f = planner.create_file(&out)?;
        let res = copy_with_progress(&mut zf, &mut f, progress);
        drop(f);
        match res {
            Ok(n) => planner.finish_file(out, n, e.modified),
            Err(err) => {
                let _ = fs::remove_file(&out);
                if e.encrypted && archive_error(&err).is_none() {
                    // AES authentication / CRC failure after a password that passed the quick check.
                    return Err(ArchiveError::WrongPassword.into());
                }
                return Err(entry_error(err, &e.path));
            }
        }
    }
    Ok(())
}

fn extract_7z(listing: &Listing, planner: &mut Planner<'_>, progress: &Progress) -> Result<()> {
    let pw = match &planner.opts.password {
        Some(p) if !p.is_empty() => sevenz_rust2::Password::new(p),
        _ => sevenz_rust2::Password::empty(),
    };
    let mut reader = sevenz_rust2::ArchiveReader::open(&listing.path, pw).map_err(sevenz_err)?;
    let mut inner_err: Option<anyhow::Error> = None;
    let res = reader.for_each_entries_with_decision(|entry, r| {
        use sevenz_rust2::EntryDecision;
        if progress.cancelled() {
            inner_err = Some(ArchiveError::Cancelled.into());
            return Ok(EntryDecision::Stop);
        }
        let path = normalize_entry_path(&entry.name);
        if path.is_empty() || entry.is_anti_item {
            return Ok(EntryDecision::Continue);
        }
        progress.set_current(&path);
        let Some(out) = planner.target(&path, entry.is_directory) else {
            progress.add(entry.size);
            return Ok(EntryDecision::Continue);
        };
        let step = (|| -> Result<()> {
            if entry.is_directory {
                planner.make_dir(&out)?;
            } else {
                let mut f = planner.create_file(&out)?;
                let n = copy_with_progress(r, &mut f, progress)?;
                drop(f);
                let mtime = if entry.has_last_modified_date { nt_to_naive(entry.last_modified_date) } else { None };
                planner.finish_file(out.clone(), n, mtime);
            }
            Ok(())
        })();
        match step {
            Ok(()) => Ok(EntryDecision::Continue),
            Err(e) => {
                let _ = fs::remove_file(&out);
                inner_err = Some(entry_error(e, &path));
                Ok(EntryDecision::Stop)
            }
        }
    });
    if let Some(e) = inner_err {
        return Err(e);
    }
    res.map_err(sevenz_err)?;
    Ok(())
}

fn extract_single(listing: &Listing, planner: &mut Planner<'_>, progress: &Progress) -> Result<()> {
    let Some(e) = listing.entries.first() else { return Ok(()) };
    progress.set_current(&e.path);
    let Some(out) = planner.target(&e.path, false) else { return Ok(()) };
    let mut d = listing.kind.filter.reader(File::open(&listing.path)?)?;
    let mut f = planner.create_file(&out)?;
    let res = copy_with_progress(&mut d, &mut f, progress);
    drop(f);
    match res {
        Ok(n) => {
            planner.finish_file(out, n, e.modified);
            Ok(())
        }
        Err(err) => {
            let _ = fs::remove_file(&out);
            Err(entry_error(err, &e.path))
        }
    }
}

// ---------------------------------------------------------------------------
// Creating and editing ZIP archives
// ---------------------------------------------------------------------------

/// A compression algorithm inside a ZIP or 7z archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Store,
    Deflate,
    Bzip2,
    Lzma,
    Lzma2,
    Xz,
    Zstd,
    Ppmd,
    Brotli,
    Lz4,
}

impl Method {
    pub fn label(self) -> &'static str {
        match self {
            Method::Store => tr!("저장 (압축 안 함)", "Store (no compression)"),
            Method::Deflate => "Deflate",
            Method::Bzip2 => "BZip2",
            Method::Lzma => "LZMA",
            Method::Lzma2 => "LZMA2",
            Method::Xz => "XZ",
            Method::Zstd => "Zstandard",
            Method::Ppmd => "PPMd",
            Method::Brotli => "Brotli",
            Method::Lz4 => "LZ4",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PackOptions {
    /// Algorithm for ZIP and 7z; other formats use their filter.
    pub method: Method,
    /// 0 = fastest (store for ZIP/7z), 9 = smallest.
    pub level: u8,
    /// AES-256 password (ZIP, 7z); `None` or empty = no encryption.
    pub password: Option<String>,
}

impl Default for PackOptions {
    fn default() -> Self {
        Self { method: Method::Deflate, level: 6, password: None }
    }
}

/// A file or folder on disk and the name it gets inside the archive.
#[derive(Debug, Clone)]
struct SourceItem {
    disk: PathBuf,
    name: String,
    is_dir: bool,
    size: u64,
}

fn collect_sources(sources: &[PathBuf], base: &str) -> Result<Vec<SourceItem>> {
    let mut out = Vec::new();
    for src in sources {
        let meta = fs::metadata(src).with_context(|| trf!("찾을 수 없습니다: {}", "Not found: {}", src.display()))?;
        let fname = src
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .ok_or_else(|| anyhow!(trf!("잘못된 경로: {}", "Invalid path: {}", src.display())))?;
        let name = if base.is_empty() { fname } else { format!("{base}/{fname}") };
        if meta.is_dir() {
            walk_dir(src, &name, &mut out)?;
        } else {
            out.push(SourceItem { disk: src.clone(), name, is_dir: false, size: meta.len() });
        }
    }
    Ok(out)
}

fn walk_dir(dir: &Path, name: &str, out: &mut Vec<SourceItem>) -> Result<()> {
    out.push(SourceItem { disk: dir.to_path_buf(), name: name.to_string(), is_dir: true, size: 0 });
    let mut items: Vec<_> = fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
    items.sort_by_key(|e| e.file_name());
    for item in items {
        let p = item.path();
        let child = format!("{name}/{}", item.file_name().to_string_lossy());
        let ft = item.file_type()?;
        if ft.is_dir() {
            walk_dir(&p, &child, out)?;
        } else if ft.is_file() {
            let size = item.metadata().map(|m| m.len()).unwrap_or(0);
            out.push(SourceItem { disk: p, name: child, is_dir: false, size });
        }
        // Symlinks and special files are skipped on purpose.
    }
    Ok(())
}

fn file_mtime(p: &Path) -> zip::DateTime {
    fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .map(|t| DateTime::<Local>::from(t).naive_local())
        .and_then(|n| zip::DateTime::try_from(n).ok())
        .unwrap_or_else(zip::DateTime::default_for_write)
}

fn write_sources<W: Write + Seek>(
    zw: &mut zip::ZipWriter<W>,
    items: &[SourceItem],
    opts: &PackOptions,
    progress: &Progress,
) -> Result<()> {
    let (method, level) = match (opts.level, opts.method) {
        (0, _) | (_, Method::Store) => (zip::CompressionMethod::Stored, None),
        (l, Method::Bzip2) => (zip::CompressionMethod::Bzip2, Some(l as i64)),
        (l, Method::Zstd) => (zip::CompressionMethod::Zstd, Some([1, 1, 2, 3, 5, 7, 9, 12, 15, 19][l.min(9) as usize])),
        (l, Method::Xz | Method::Lzma | Method::Lzma2) => (zip::CompressionMethod::Xz, Some(l as i64)),
        (l, Method::Ppmd) => (zip::CompressionMethod::Ppmd, Some(l as i64)),
        (l, _) => (zip::CompressionMethod::Deflated, Some(l as i64)),
    };
    let password = opts.password.clone().filter(|p| !p.is_empty());
    for item in items {
        progress.check()?;
        progress.set_current(&item.name);
        let mut fo: zip::write::SimpleFileOptions = zip::write::FileOptions::default()
            .compression_method(method)
            .last_modified_time(file_mtime(&item.disk))
            .large_file(item.size >= 0xFFFF_FFFF);
        if let Some(l) = level {
            fo = fo.compression_level(Some(l.min(19)));
        }
        if item.is_dir {
            zw.add_directory(format!("{}/", item.name), fo)?;
            continue;
        }
        match &password {
            Some(pw) => {
                let fo = fo.with_aes_encryption(zip::AesMode::Aes256, pw.as_str());
                zw.start_file(item.name.as_str(), fo)?;
            }
            None => zw.start_file(item.name.as_str(), fo)?,
        }
        let mut f = File::open(&item.disk).with_context(|| trf!("읽을 수 없습니다: {}", "Cannot read: {}", item.disk.display()))?;
        copy_with_progress(&mut f, zw, progress)?;
    }
    Ok(())
}

fn temp_beside(dest: &Path) -> Result<tempfile::NamedTempFile> {
    let dir = dest.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    tempfile::Builder::new()
        .prefix(".rustzip-")
        .suffix(".tmp")
        .tempfile_in(dir)
        .with_context(|| trf!("임시 파일을 만들 수 없습니다: {}", "Cannot create a temporary file in: {}", dir.display()))
}

/// Creates a new archive of type `kind` at `dest` from files and folders.
/// Returns the number of files stored.
pub fn create(dest: &Path, sources: &[PathBuf], kind: ArchiveKind, opts: &PackOptions, progress: &Progress) -> Result<usize> {
    if sources.is_empty() {
        bail!(tr!("압축할 파일을 선택하세요.", "Choose files to compress."));
    }
    let dest_abs = dest.canonicalize().unwrap_or_else(|_| dest.to_path_buf());
    let items: Vec<SourceItem> = collect_sources(sources, "")?
        .into_iter()
        .filter(|i| i.disk.canonicalize().map(|c| c != dest_abs).unwrap_or(true))
        .collect();
    progress.set_total(items.iter().map(|i| i.size).sum::<u64>().max(1));
    let tmp = temp_beside(dest)?;
    match (kind.container, kind.filter) {
        (Container::Zip, Filter::None) => {
            let mut zw = zip::ZipWriter::new(io::BufWriter::new(tmp.as_file()));
            write_sources(&mut zw, &items, opts, progress)?;
            zw.finish()?.flush()?;
        }
        (Container::SevenZ, Filter::None) => create::sevenz(tmp.as_file(), &items, opts, progress)?,
        (Container::Tar, f) if f.can_write() => create::tar(tmp.as_file(), &items, f, opts.level, progress)?,
        (Container::Raw, f) if f != Filter::None && f.can_write() => create::single(tmp.as_file(), &items, f, opts.level, progress)?,
        _ => return Err(ArchiveError::Unsupported(trf!("{} 형식으로 압축하기", "creating {} archives", kind.label())).into()),
    }
    tmp.persist(dest).map_err(|e| anyhow::Error::new(e.error).context(tr!("압축 파일을 저장할 수 없습니다.", "Cannot save the archive.")))?;
    Ok(items.iter().filter(|i| !i.is_dir).count())
}

/// Rewrites a ZIP: keeps entries for which `keep(index, path)` is true (raw copy,
/// so names, encryption and compression are preserved), then appends `new_items`.
fn rewrite_zip(
    listing: &Listing,
    keep: impl Fn(&Entry) -> bool,
    new_items: &[SourceItem],
    opts: &PackOptions,
    progress: &Progress,
) -> Result<()> {
    if listing.kind != ArchiveKind::ZIP {
        return Err(ArchiveError::Unsupported(tr!("ZIP 파일만 수정할 수 있습니다", "only ZIP files can be changed").into()).into());
    }
    let mut src = zip::ZipArchive::new(BufReader::new(File::open(&listing.path)?))?;
    let kept: HashSet<usize> = listing.entries.iter().filter(|e| keep(e)).map(|e| e.index).collect();
    let copy_bytes: u64 = listing.entries.iter().filter(|e| kept.contains(&e.index)).filter_map(|e| e.packed).sum();
    progress.set_total((copy_bytes + new_items.iter().map(|i| i.size).sum::<u64>()).max(1));
    let tmp = temp_beside(&listing.path)?;
    {
        let mut zw = zip::ZipWriter::new(io::BufWriter::new(tmp.as_file()));
        for i in 0..src.len() {
            // Entries that the listing skipped (empty names) are kept as they were.
            let listed = listing.entries.iter().any(|e| e.index == i);
            if listed && !kept.contains(&i) {
                continue;
            }
            progress.check()?;
            let f = src.by_index_raw(i)?;
            progress.add(f.compressed_size());
            zw.raw_copy_file(f)?;
        }
        write_sources(&mut zw, new_items, opts, progress)?;
        zw.finish()?.flush()?;
    }
    tmp.persist(&listing.path).map_err(|e| anyhow::Error::new(e.error).context(tr!("압축 파일을 저장할 수 없습니다.", "Cannot save the archive.")))?;
    Ok(())
}

/// Adds files/folders into the folder `base` ("" = root) of an existing ZIP.
/// Entries with the same name are replaced.
pub fn add_to_zip(listing: &Listing, sources: &[PathBuf], base: &str, opts: &PackOptions, progress: &Progress) -> Result<usize> {
    let items = collect_sources(sources, base)?;
    let new_names: HashSet<String> = items.iter().map(|i| i.name.clone()).collect();
    rewrite_zip(listing, |e| !new_names.contains(&e.path), &items, opts, progress)?;
    Ok(items.iter().filter(|i| !i.is_dir).count())
}

/// Deletes the selected files/folders (paths as shown in the listing) from a ZIP.
pub fn delete_from_zip(listing: &Listing, selection: &[String], progress: &Progress) -> Result<usize> {
    let removed = listing.entries.iter().filter(|e| is_selected(&e.path, selection)).count();
    if removed == 0 {
        bail!(tr!("삭제할 항목이 없습니다.", "Nothing to delete."));
    }
    rewrite_zip(listing, |e| !is_selected(&e.path, selection), &[], &PackOptions::default(), progress)?;
    Ok(removed)
}

// ---------------------------------------------------------------------------
// Formatting helpers shared with the GUI
// ---------------------------------------------------------------------------

pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// Packed / original as a percentage ("압축률").
pub fn ratio(size: u64, packed: Option<u64>) -> Option<f32> {
    let p = packed?;
    if size == 0 {
        return None;
    }
    Some((p as f64 / size as f64 * 100.0) as f32)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod formats_tests;

#[cfg(all(test, unix))]
mod interop;
