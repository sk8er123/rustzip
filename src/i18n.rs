//! Interface language (Korean / English).
//!
//! Every piece of text the person can see goes through one of two macros:
//!   `tr!("한국어", "English")`               -> `&'static str`
//!   `trf!("파일 {n}개", "{n} files", ...)`   -> `String` (format! rules)
//! Both languages sit side by side at the call site, and the language is
//! read when the text is made, so worker threads produce messages in the
//! language that is active at that moment.

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Ko = 0,
    En = 1,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::Ko, Lang::En];

    /// The language's own name, so people can find it whatever is shown now.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::Ko => "한국어",
            Lang::En => "English",
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Lang::Ko => "ko",
            Lang::En => "en",
        }
    }

    /// Accepts "ko", "ko_KR.UTF-8", "ko-KR", "en", "en_US", ...
    pub fn from_code(s: &str) -> Option<Lang> {
        let s = s.trim().to_ascii_lowercase();
        if s.starts_with("ko") {
            Some(Lang::Ko)
        } else if s.starts_with("en") {
            Some(Lang::En)
        } else {
            None
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(Lang::Ko as u8);

thread_local! {
    /// Per-thread override, so tests can check English text without
    /// changing the language other tests running in parallel see.
    static OVERRIDE: Cell<Option<Lang>> = const { Cell::new(None) };
}

pub fn lang() -> Lang {
    if let Some(l) = OVERRIDE.with(Cell::get) {
        return l;
    }
    match CURRENT.load(Ordering::Relaxed) {
        1 => Lang::En,
        _ => Lang::Ko,
    }
}

pub fn set_lang(l: Lang) {
    CURRENT.store(l as u8, Ordering::Relaxed);
}

/// Runs `f` with this thread's language set to `l`.
#[cfg(test)]
pub fn with_lang<T>(l: Lang, f: impl FnOnce() -> T) -> T {
    let prev = OVERRIDE.with(|c| c.replace(Some(l)));
    let out = f();
    OVERRIDE.with(|c| c.set(prev));
    out
}

/// True when the text has no `{` / `}` (checked at compile time by `tr!`).
pub const fn is_plain(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'{' || b[i] == b'}' {
            return false;
        }
        i += 1;
    }
    true
}

/// Fixed text in the current language.
macro_rules! tr {
    ($ko:literal, $en:literal $(,)?) => {{
        const _: () = assert!(
            $crate::i18n::is_plain($ko) && $crate::i18n::is_plain($en),
            "text with {{}} placeholders must use trf!"
        );
        match $crate::i18n::lang() {
            $crate::i18n::Lang::Ko => $ko,
            $crate::i18n::Lang::En => $en,
        }
    }};
}

/// Formatted text in the current language (same rules as `format!`).
macro_rules! trf {
    ($ko:literal, $en:literal $(, $arg:expr)* $(,)?) => {
        match $crate::i18n::lang() {
            $crate::i18n::Lang::Ko => format!($ko $(, $arg)*),
            $crate::i18n::Lang::En => format!($en $(, $arg)*),
        }
    };
}

// ---------------------------------------------------------------------------
// Choosing the language at start-up and remembering the person's choice
// ---------------------------------------------------------------------------

/// The operating system's language, or English when it is neither Korean nor English.
pub fn system_lang() -> Lang {
    sys_locale::get_locales().find_map(|l| Lang::from_code(&l)).unwrap_or(Lang::En)
}

fn settings_path() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let dir = if cfg!(windows) {
        env("APPDATA")?.join("RustZip")
    } else if cfg!(target_os = "macos") {
        env("HOME")?.join("Library/Application Support/RustZip")
    } else {
        env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config")))?.join("rustzip")
    };
    Some(dir.join("settings.txt"))
}

/// The language the person picked last time, if any.
pub fn saved_lang() -> Option<Lang> {
    let text = std::fs::read_to_string(settings_path()?).ok()?;
    text.lines()
        .filter_map(|l| l.split_once('='))
        .find(|(k, _)| k.trim() == "lang")
        .and_then(|(_, v)| Lang::from_code(v))
}

pub fn save_lang(l: Lang) -> std::io::Result<()> {
    let Some(path) = settings_path() else { return Ok(()) };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, format!("lang={}\n", l.code()))
}

// ---------------------------------------------------------------------------
// Error messages
// ---------------------------------------------------------------------------

/// Turns an error into text for the person, in the current language.
///
/// RustZip's own messages are already translated. Errors that come from the
/// libraries underneath (file system, zip, 7z, xz, ...) have English text, so
/// they are recognised by type and kind and described in the current language.
/// Only an error nothing here recognises keeps its original text, after a
/// translated lead-in, so no information is lost.
pub fn describe_error(err: &anyhow::Error) -> String {
    let mut parts: Vec<String> = Vec::new();
    for cause in err.chain() {
        let known = describe_known(cause);
        let done = known.is_some();
        let text = known.unwrap_or_else(|| cause.to_string());
        if parts.last() != Some(&text) {
            parts.push(text);
        }
        if done {
            break;
        }
    }
    parts.join("\n")
}

fn describe_known(e: &(dyn std::error::Error + 'static)) -> Option<String> {
    if let Some(a) = e.downcast_ref::<crate::engine::ArchiveError>() {
        return Some(a.to_string());
    }
    if let Some(io) = e.downcast_ref::<std::io::Error>() {
        return Some(describe_io(io));
    }
    if let Some(z) = e.downcast_ref::<zip::result::ZipError>() {
        return Some(describe_zip(z));
    }
    if let Some(s) = e.downcast_ref::<sevenz_rust2::Error>() {
        return Some(describe_7z(s));
    }
    None
}

fn describe_io(e: &std::io::Error) -> String {
    use std::io::ErrorKind as K;
    let s = match e.kind() {
        K::UnexpectedEof => tr!(
            "파일이 중간에 끊겨 있습니다. 내려받기가 덜 되었거나 손상된 파일입니다.",
            "The file ends too early. It was not fully downloaded or is damaged."
        ),
        K::InvalidData => tr!(
            "데이터가 손상되었습니다. 검사값이 맞지 않거나 형식이 올바르지 않습니다.",
            "The data is damaged: a checksum does not match or the format is invalid."
        ),
        K::NotFound => tr!("파일이나 폴더를 찾을 수 없습니다.", "The file or folder was not found."),
        K::PermissionDenied => tr!("접근 권한이 없습니다.", "Permission denied."),
        K::AlreadyExists => tr!("같은 이름의 파일이 이미 있습니다.", "A file with that name already exists."),
        K::StorageFull => tr!("디스크 공간이 부족합니다.", "The disk is full."),
        K::ReadOnlyFilesystem => tr!("읽기 전용 위치라 쓸 수 없습니다.", "The location is read-only."),
        K::IsADirectory => tr!("파일 대신 폴더가 지정되었습니다.", "A folder was given where a file was expected."),
        K::NotADirectory => tr!("폴더 대신 파일이 지정되었습니다.", "A file was given where a folder was expected."),
        K::InvalidFilename => tr!("이 시스템에서 쓸 수 없는 파일 이름입니다.", "The file name is not allowed on this system."),
        K::OutOfMemory => tr!("메모리가 부족합니다.", "Out of memory."),
        _ => {
            return trf!(
                "파일을 읽거나 쓰는 중 오류가 발생했습니다. (자세한 정보: {e})",
                "An error occurred while reading or writing a file. (Details: {e})"
            )
        }
    };
    s.to_string()
}

fn describe_zip(e: &zip::result::ZipError) -> String {
    use zip::result::ZipError as Z;
    match e {
        Z::Io(io) => describe_io(io),
        Z::InvalidArchive(_) => tr!(
            "ZIP 파일 구조가 손상되었거나 ZIP 파일이 아닙니다.",
            "The ZIP structure is damaged, or this is not a ZIP file."
        )
        .into(),
        Z::UnsupportedArchive(what) => trf!(
            "지원하지 않는 ZIP 기능을 사용한 파일입니다. ({what})",
            "This ZIP uses a feature that is not supported. ({what})"
        ),
        Z::CompressionMethodNotSupported(m) => trf!(
            "지원하지 않는 압축 방식입니다. (방식 번호 {m})",
            "Unsupported compression method (method {m})."
        ),
        Z::FileNotFound => tr!("압축 파일 안에서 항목을 찾을 수 없습니다.", "The entry was not found in the archive.").into(),
        Z::InvalidPassword => crate::engine::ArchiveError::WrongPassword.to_string(),
        other => trf!("ZIP 파일을 처리할 수 없습니다. (자세한 정보: {other})", "Could not process the ZIP file. (Details: {other})"),
    }
}

fn describe_7z(e: &sevenz_rust2::Error) -> String {
    use sevenz_rust2::Error as S;
    match e {
        S::Io(io, _) | S::FileOpen(io, _) => describe_io(io),
        S::BadSignature(_) => tr!("7z 파일이 아니거나 머리 부분이 손상되었습니다.", "This is not a 7z file, or its header is damaged.").into(),
        S::ChecksumVerificationFailed | S::NextHeaderCrcMismatch => tr!(
            "데이터가 손상되었습니다. 검사값이 맞지 않습니다.",
            "The data is damaged: a checksum does not match."
        )
        .into(),
        S::UnsupportedVersion { major, minor } => trf!(
            "지원하지 않는 7z 버전입니다. ({major}.{minor})",
            "Unsupported 7z version ({major}.{minor})."
        ),
        S::PasswordRequired => crate::engine::ArchiveError::PasswordRequired.to_string(),
        S::MaybeBadPassword(_) => crate::engine::ArchiveError::WrongPassword.to_string(),
        other => trf!("7z 파일을 처리할 수 없습니다. (자세한 정보: {other})", "Could not process the 7z file. (Details: {other})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context as _;

    #[test]
    fn language_codes() {
        assert_eq!(Lang::from_code("ko_KR.UTF-8"), Some(Lang::Ko));
        assert_eq!(Lang::from_code("ko-KR"), Some(Lang::Ko));
        assert_eq!(Lang::from_code("en_US"), Some(Lang::En));
        assert_eq!(Lang::from_code("ja_JP"), None);
    }

    #[test]
    fn macros_follow_the_thread_language() {
        let n = 3;
        assert_eq!(with_lang(Lang::Ko, || tr!("취소", "Cancel")), "취소");
        assert_eq!(with_lang(Lang::En, || tr!("취소", "Cancel")), "Cancel");
        assert_eq!(with_lang(Lang::Ko, || trf!("파일 {n}개", "{n} files")), "파일 3개");
        assert_eq!(with_lang(Lang::En, || trf!("파일 {n}개", "{n} files")), "3 files");
    }

    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn chosen_language_is_remembered() {
        let dir = tempfile::tempdir().unwrap();
        // Only this test reads XDG_CONFIG_HOME, so changing it here is safe.
        std::env::set_var("XDG_CONFIG_HOME", dir.path());
        assert_eq!(saved_lang(), None);
        save_lang(Lang::En).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("rustzip/settings.txt")).unwrap(), "lang=en\n");
        assert_eq!(saved_lang(), Some(Lang::En));
        save_lang(Lang::Ko).unwrap();
        assert_eq!(saved_lang(), Some(Lang::Ko));
    }

    #[test]
    fn library_errors_are_translated() {
        let eof = || anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
        let crc = || anyhow::Error::new(std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid checksum"));
        let ko = with_lang(Lang::Ko, || describe_error(&eof()));
        assert!(ko.contains("끊겨") && !ko.contains("failed to fill"), "{ko}");
        let ko = with_lang(Lang::Ko, || describe_error(&crc().context("'a.txt' 항목을 풀 수 없습니다.")));
        assert_eq!(ko, "'a.txt' 항목을 풀 수 없습니다.\n데이터가 손상되었습니다. 검사값이 맞지 않거나 형식이 올바르지 않습니다.");
        let en = with_lang(Lang::En, || describe_error(&crc()));
        assert!(en.starts_with("The data is damaged"), "{en}");
        let z = anyhow::Error::new(zip::result::ZipError::InvalidArchive("bad".into()));
        assert!(with_lang(Lang::Ko, || describe_error(&z)).starts_with("ZIP 파일 구조가"));
    }
}
