//! Compatibility with other archive tools: archives made by Info-ZIP `zip`,
//! GNU `tar`, `gzip`/`bzip2`/`xz` and Python's `zipfile` open in RustZip, and
//! ZIPs made by RustZip pass `unzip -t` and Python's checks.
//!
//! These tests run the real tools. When one is not installed the test prints
//! why and skips instead of failing, so `cargo test` still works on machines
//! without them. Unix only.
//!
//! The 200 MB test is slow in debug builds and is skipped by default:
//!   cargo test --release -- --ignored big_file

use super::*;
use std::collections::BTreeMap;
use std::process::Command;

use crate::i18n::{describe_error, with_lang, Lang};

fn p() -> Arc<Progress> {
    Progress::new()
}

/// True when every program is on PATH; otherwise says which one is missing.
fn have(tools: &[&str]) -> bool {
    for tool in tools {
        let found = std::env::var_os("PATH")
            .is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(tool).is_file()));
        if !found {
            eprintln!("건너뜀(skipped): '{tool}' is not installed");
            return false;
        }
    }
    true
}

/// Runs a program in `dir`, panicking with its output if it fails.
fn run(dir: &Path, program: &str, args: &[&str]) -> String {
    let out = Command::new(program).args(args).current_dir(dir).output().unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{program} {args:?} failed:\n{text}");
    text
}

/// Runs a program and writes its standard output to `dest` (for `gzip -c` etc.).
fn run_to_file(dir: &Path, program: &str, args: &[&str], dest: &Path) {
    let out = Command::new(program).args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "{program} {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    fs::write(dest, out.stdout).unwrap();
}

fn python(dir: &Path, code: &str) -> String {
    run(dir, "python3", &["-I", "-c", code])
}

/// Sample tree `root/자료` with Korean names, an empty folder, an empty file,
/// text that compresses well and noise that does not.
fn tree(root: &Path) -> PathBuf {
    let src = root.join("자료");
    fs::create_dir_all(src.join("문서/깊은 폴더")).unwrap();
    fs::create_dir_all(src.join("빈 폴더")).unwrap();
    fs::write(src.join("hello.txt"), "hello\n".repeat(1000)).unwrap();
    fs::write(src.join("빈파일.txt"), b"").unwrap();
    fs::write(src.join("문서/보고서.txt"), "한글 내용 ".repeat(5000)).unwrap();
    let mut x: u64 = 88172645463325252;
    let noise: Vec<u8> = (0..300_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    fs::write(src.join("문서/깊은 폴더/noise.bin"), noise).unwrap();
    src
}

fn extract_to(listing: &Listing, dest: &Path, password: Option<&str>) -> Result<ExtractReport> {
    let opts = ExtractOptions {
        dest: dest.into(),
        selection: None,
        strip_prefix: String::new(),
        password: password.map(Into::into),
        overwrite: Overwrite::Always,
    };
    extract(listing, &opts, &p())
}

/// Every file and folder below `dir`: relative path -> contents (`None` for folders).
fn snapshot(dir: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, Option<Vec<u8>>>) {
        for e in fs::read_dir(dir).unwrap().flatten() {
            let path = e.path();
            let rel = path.strip_prefix(base).unwrap().to_string_lossy().into_owned();
            if path.is_dir() {
                out.insert(rel, None);
                walk(base, &path, out);
            } else {
                out.insert(rel, Some(fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn assert_same_tree(expected: &Path, actual: &Path) {
    let (a, b) = (snapshot(expected), snapshot(actual));
    let names = |m: &BTreeMap<String, Option<Vec<u8>>>| m.keys().cloned().collect::<Vec<_>>();
    assert_eq!(names(&a), names(&b), "different files in {}", actual.display());
    for (k, v) in &a {
        assert!(b[k] == *v, "'{k}' differs in {}", actual.display());
    }
}

#[test]
fn infozip_archive_opens_and_extracts() {
    if !have(&["zip"]) {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let src = tree(t.path());
    run(t.path(), "zip", &["-qr", "infozip.zip", "자료"]);
    let l = list(&t.path().join("infozip.zip"), NameEncoding::Auto, &p()).unwrap();
    assert_eq!(l.kind, ArchiveKind::ZIP);
    assert_eq!(l.file_count(), 4);
    extract_to(&l, &t.path().join("out"), None).unwrap();
    assert_same_tree(&src, &t.path().join("out/자료"));
}

#[test]
fn infozip_zipcrypto_password() {
    if !have(&["zip"]) {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let src = tree(t.path());
    run(t.path(), "zip", &["-qr", "-P", "비번123", "enc.zip", "자료"]);
    let l = list(&t.path().join("enc.zip"), NameEncoding::Auto, &p()).unwrap();
    assert!(l.any_encrypted());

    let none = extract_to(&l, &t.path().join("none"), None).unwrap_err();
    assert_eq!(archive_error(&none), Some(&ArchiveError::PasswordRequired));
    let wrong = extract_to(&l, &t.path().join("wrong"), Some("틀림")).unwrap_err();
    assert_eq!(archive_error(&wrong), Some(&ArchiveError::WrongPassword));

    extract_to(&l, &t.path().join("out"), Some("비번123")).unwrap();
    assert_same_tree(&src, &t.path().join("out/자료"));
}

/// A ZIP made on Korean Windows: CP949 names without the UTF-8 flag.
#[test]
fn windows_cp949_zip_from_python() {
    if !have(&["python3"]) {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    python(
        t.path(),
        r#"
import zipfile
class CP949(zipfile.ZipInfo):
    def _encodeFilenameFlags(self):
        return self.filename.encode('cp949'), self.flag_bits
with zipfile.ZipFile('win.zip', 'w', zipfile.ZIP_DEFLATED) as z:
    for name, data in [('결재 서류/2026년 예산.xlsx', b'x' * 5000), ('똠방각하.txt', b'y' * 300)]:
        info = CP949(name)
        info.compress_type = zipfile.ZIP_DEFLATED
        z.writestr(info, data)
assert all(i.flag_bits & 0x800 == 0 for i in zipfile.ZipFile('win.zip').infolist())
"#,
    );
    let l = list(&t.path().join("win.zip"), NameEncoding::Auto, &p()).unwrap();
    let names: Vec<&str> = l.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(names, ["결재 서류/2026년 예산.xlsx", "똠방각하.txt"]);
    extract_to(&l, &t.path().join("out"), None).unwrap();
    assert_eq!(fs::read(t.path().join("out/결재 서류/2026년 예산.xlsx")).unwrap(), vec![b'x'; 5000]);
}

#[test]
fn rustzip_zip_passes_unzip_and_python() {
    if !have(&["unzip", "python3"]) {
        return;
    }
    for level in [0u8, 1, 6, 9] {
        let t = tempfile::tempdir().unwrap();
        let src = tree(t.path());
        create(&t.path().join("rz.zip"), &[src.clone()], ArchiveKind::ZIP, &PackOptions { level, password: None, ..Default::default() }, &p()).unwrap();
        run(t.path(), "unzip", &["-tq", "rz.zip"]);
        fs::create_dir(t.path().join("u")).unwrap();
        run(&t.path().join("u"), "unzip", &["-q", "../rz.zip"]);
        assert_same_tree(&src, &t.path().join("u/자료"));
        let out = python(
            t.path(),
            "import zipfile; z = zipfile.ZipFile('rz.zip'); print(z.testzip()); print(sorted(i.filename for i in z.infolist()))",
        );
        assert!(out.starts_with("None\n"), "level {level}: {out}");
        assert!(out.contains("'자료/문서/보고서.txt'"), "level {level}: names not UTF-8: {out}");
    }
}

/// RustZip's passwords are WinZip AES-256 (AE-2). Python sees the entries as
/// encrypted AES (method 99); Info-ZIP unzip 6.0 lists them but cannot decrypt.
#[test]
fn rustzip_aes_zip_is_marked_aes_for_other_tools() {
    if !have(&["unzip", "python3"]) {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let src = tree(t.path());
    create(&t.path().join("aes.zip"), &[src], ArchiveKind::ZIP, &PackOptions { level: 6, password: Some("pw".into()), ..Default::default() }, &p()).unwrap();
    let out = python(
        t.path(),
        "import zipfile; z = zipfile.ZipFile('aes.zip'); \
         print(sorted({(i.flag_bits & 1, i.compress_type) for i in z.infolist() if not i.is_dir()}))",
    );
    assert_eq!(out.trim(), "[(1, 99)]");
    let listing = run(t.path(), "unzip", &["-l", "aes.zip"]);
    assert!(listing.contains("8 files"), "{listing}");
}

#[test]
fn add_and_delete_keep_an_infozip_archive_valid() {
    if !have(&["zip", "unzip"]) {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    tree(t.path());
    run(t.path(), "zip", &["-qr", "a.zip", "자료"]);
    let z = t.path().join("a.zip");
    let extra = t.path().join("추가.txt");
    fs::write(&extra, "추가 파일").unwrap();

    let l = list(&z, NameEncoding::Auto, &p()).unwrap();
    add_to_zip(&l, &[extra], "자료/문서", &PackOptions::default(), &p()).unwrap();
    let l = list(&z, NameEncoding::Auto, &p()).unwrap();
    delete_from_zip(&l, &["자료/문서/깊은 폴더".into()], &p()).unwrap();

    run(t.path(), "unzip", &["-tq", "a.zip"]);
    let names = run(t.path(), "unzip", &["-Z1", "a.zip"]);
    assert!(names.lines().any(|n| n == "자료/문서/추가.txt"), "{names}");
    assert!(!names.contains("noise.bin") && !names.contains("깊은 폴더"), "{names}");
}

#[test]
fn gnu_tar_archives() {
    if !have(&["tar", "gzip", "bzip2", "xz"]) {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let src = tree(t.path());
    let cases = [
        ("a.tar", "-cf", ArchiveKind::TAR),
        ("a.tar.gz", "-czf", ArchiveKind::new(Container::Tar, Filter::Gzip)),
        ("a.tgz", "-czf", ArchiveKind::new(Container::Tar, Filter::Gzip)),
        ("a.tar.bz2", "-cjf", ArchiveKind::new(Container::Tar, Filter::Bzip2)),
        ("a.tar.xz", "-cJf", ArchiveKind::new(Container::Tar, Filter::Xz)),
    ];
    for (name, flags, kind) in cases {
        run(t.path(), "tar", &[flags, name, "자료"]);
        let l = list(&t.path().join(name), NameEncoding::Auto, &p()).unwrap();
        assert_eq!(l.kind, kind, "{name}");
        let out = t.path().join(format!("out-{name}"));
        extract_to(&l, &out, None).unwrap();
        assert_same_tree(&src, &out.join("자료"));
    }
}

#[test]
fn single_compressed_files() {
    if !have(&["gzip", "bzip2", "xz"]) {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let orig = tree(t.path()).join("문서/보고서.txt");
    let orig_arg = orig.to_str().unwrap();
    for (tool, ext, kind) in [("gzip", "gz", ArchiveKind::new(Container::Raw, Filter::Gzip)), ("xz", "xz", ArchiveKind::new(Container::Raw, Filter::Xz)), ("bzip2", "bz2", ArchiveKind::new(Container::Raw, Filter::Bzip2))] {
        let f = t.path().join(format!("보고서.txt.{ext}"));
        run_to_file(t.path(), tool, &["-c", orig_arg], &f);
        let l = list(&f, NameEncoding::Auto, &p()).unwrap();
        assert_eq!(l.kind, kind);
        assert_eq!(l.entries[0].path, "보고서.txt");
        assert_eq!(l.entries[0].size, fs::metadata(&orig).unwrap().len(), "{ext}");
        let r = extract_to(&l, &t.path().join(format!("out-{ext}")), None).unwrap();
        assert_eq!(fs::read(&r.written[0]).unwrap(), fs::read(&orig).unwrap(), "{ext}");
    }
}

#[test]
fn format_is_detected_by_content_not_extension() {
    if !have(&["zip", "tar", "gzip"]) {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    tree(t.path());
    run(t.path(), "zip", &["-qr", "real.zip", "자료"]);
    run(t.path(), "tar", &["-czf", "real.tgz", "자료"]);
    fs::copy(t.path().join("real.zip"), t.path().join("그림.jpg")).unwrap();
    fs::copy(t.path().join("real.tgz"), t.path().join("이상한.zip")).unwrap();
    assert_eq!(list(&t.path().join("그림.jpg"), NameEncoding::Auto, &p()).unwrap().kind, ArchiveKind::ZIP);
    assert_eq!(list(&t.path().join("이상한.zip"), NameEncoding::Auto, &p()).unwrap().kind, ArchiveKind::new(Container::Tar, Filter::Gzip));
}

/// Damaged files give an error (never a panic or a silent success), and the
/// message is in the interface language rather than the libraries' English.
#[test]
fn damaged_archives_give_translated_errors() {
    if !have(&["zip", "tar", "xz"]) {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    tree(t.path());
    let dir = t.path();
    run(dir, "zip", &["-qr", "good.zip", "자료"]);
    run(dir, "tar", &["-cJf", "good.tar.xz", "자료"]);
    let zip = fs::read(dir.join("good.zip")).unwrap();
    let xz = fs::read(dir.join("good.tar.xz")).unwrap();
    let mut flipped = zip.clone();
    for b in &mut flipped[2000..2100] {
        *b ^= 0xFF;
    }
    let junk: Vec<u8> = (0..100u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
    let cases: [(&str, &[u8]); 5] = [
        ("trunc.zip", &zip[..20_000]),
        ("trunc.tar.xz", &xz[..3_000]),
        ("junk.zip", &junk),
        ("empty.zip", &[]),
        ("flipped.zip", &flipped),
    ];
    for (name, bytes) in cases {
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        let attempt = || {
            let err = match list(&path, NameEncoding::Auto, &p()) {
                Err(e) => e,
                Ok(l) => extract_to(&l, &dir.join(format!("out-{name}")), None).expect_err(name),
            };
            describe_error(&err)
        };
        let ko = with_lang(Lang::Ko, attempt);
        let en = with_lang(Lang::En, attempt);
        assert!(ko.chars().any(|c| ('가'..='힣').contains(&c)), "{name}: {ko}");
        // English text may still contain the Korean entry name, so check how it starts.
        assert!(en.starts_with(|c: char| c.is_ascii_uppercase()) && en != ko, "{name}: {en}");
        for raw in ["failed to fill whole buffer", "Invalid checksum", "invalid Zip archive"] {
            assert!(!ko.contains(raw) && !en.contains(raw), "{name}: library text leaked: {ko} / {en}");
        }
    }
}

#[test]
#[ignore = "200 MB; run with: cargo test --release -- --ignored big_file"]
fn big_file_roundtrip() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path().join("big");
    fs::create_dir_all(&d).unwrap();
    let data: Vec<u8> = (0..200_000_000u32).map(|i| ((i / 7) % 97) as u8).collect();
    fs::write(d.join("big.bin"), &data).unwrap();
    let start = std::time::Instant::now();
    create(&t.path().join("big.zip"), &[d], ArchiveKind::ZIP, &PackOptions::default(), &p()).unwrap();
    let created = start.elapsed();
    let l = list(&t.path().join("big.zip"), NameEncoding::Auto, &p()).unwrap();
    let start = std::time::Instant::now();
    extract_to(&l, &t.path().join("out"), None).unwrap();
    eprintln!("200 MB: compress {created:?}, extract {:?}", start.elapsed());
    assert!(fs::read(t.path().join("out/big/big.bin")).unwrap() == data);
}
