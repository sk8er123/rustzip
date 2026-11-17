use super::*;
use std::io::Write;

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn p() -> Arc<Progress> {
    Progress::new()
}

/// Builds a small source tree:
///   src/hello.txt
///   src/문서/보고서.txt
///   src/문서/깊은/data.bin
fn make_tree(root: &Path) -> PathBuf {
    let src = root.join("src");
    fs::create_dir_all(src.join("문서/깊은")).unwrap();
    fs::write(src.join("hello.txt"), "hello world\n".repeat(200)).unwrap();
    fs::write(src.join("문서/보고서.txt"), "한글 내용입니다.".repeat(100)).unwrap();
    fs::write(src.join("문서/깊은/data.bin"), (0..5000u32).map(|i| (i % 251) as u8).collect::<Vec<_>>()).unwrap();
    src
}

fn read(p: &Path) -> Vec<u8> {
    fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn extract_all(listing: &Listing, dest: &Path, password: Option<&str>) -> Result<ExtractReport> {
    extract(
        listing,
        &ExtractOptions {
            dest: dest.to_path_buf(),
            selection: None,
            strip_prefix: String::new(),
            password: password.map(str::to_string),
            overwrite: Overwrite::Always,
        },
        &p(),
    )
}

/// Writes a minimal stored ZIP with raw (undecoded) names and the given flags.
fn raw_zip(path: &Path, files: &[(&[u8], &[u8])], flags: u16) {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let mut crc = flate2::Crc::new();
        crc.update(data);
        let crc = crc.sum();
        let offset = out.len() as u32;
        out.extend_from_slice(&0x04034b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // stored
        out.extend_from_slice(&0u16.to_le_bytes()); // time
        out.extend_from_slice(&0x5A21u16.to_le_bytes()); // date 2025-01-01
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(data);

        central.extend_from_slice(&0x02014b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // made by (DOS)
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&flags.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0x5A21u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attr
        central.extend_from_slice(&0u32.to_le_bytes()); // external attr
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x06054b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    fs::write(path, out).unwrap();
}

fn cp949(s: &str) -> Vec<u8> {
    let (b, _, err) = encoding_rs::EUC_KR.encode(s);
    assert!(!err);
    b.into_owned()
}

#[test]
fn zip_roundtrip_keeps_tree_and_contents() {
    let t = tmp();
    let src = make_tree(t.path());
    let zip_path = t.path().join("out.zip");
    let n = create_zip(&zip_path, &[src.clone()], &ZipOptions::default(), &p()).unwrap();
    assert_eq!(n, 3);

    let listing = list(&zip_path, NameEncoding::Auto, &p()).unwrap();
    assert_eq!(listing.kind, ArchiveKind::Zip);
    assert_eq!(listing.file_count(), 3);
    assert!(listing.entries.iter().any(|e| e.path == "src/문서/보고서.txt" && !e.is_dir));
    let e = listing.entries.iter().find(|e| e.path == "src/hello.txt").unwrap();
    assert_eq!(e.size, 2400);
    assert!(e.packed.unwrap() < e.size, "deflate should shrink repetitive text");
    assert!(e.modified.is_some());

    let dest = t.path().join("dest");
    let rep = extract_all(&listing, &dest, None).unwrap();
    assert_eq!(rep.files, 3);
    assert_eq!(read(&dest.join("src/문서/보고서.txt")), read(&src.join("문서/보고서.txt")));
    assert_eq!(read(&dest.join("src/문서/깊은/data.bin")), read(&src.join("문서/깊은/data.bin")));
}

#[test]
fn compression_level_zero_stores_and_nine_shrinks() {
    let t = tmp();
    let f = t.path().join("big.txt");
    fs::write(&f, "abcdefghij".repeat(10_000)).unwrap();
    let z0 = t.path().join("l0.zip");
    let z9 = t.path().join("l9.zip");
    create_zip(&z0, &[f.clone()], &ZipOptions { level: 0, password: None }, &p()).unwrap();
    create_zip(&z9, &[f.clone()], &ZipOptions { level: 9, password: None }, &p()).unwrap();
    let l0 = list(&z0, NameEncoding::Auto, &p()).unwrap();
    let l9 = list(&z9, NameEncoding::Auto, &p()).unwrap();
    assert_eq!(l0.entries[0].packed, Some(100_000));
    assert_eq!(l0.entries[0].method, "저장");
    assert!(l9.entries[0].packed.unwrap() < 2_000);
}

#[test]
fn aes_password_required_wrong_and_right() {
    let t = tmp();
    let src = make_tree(t.path());
    let zip_path = t.path().join("secret.zip");
    create_zip(&zip_path, &[src.clone()], &ZipOptions { level: 6, password: Some("비밀1234".into()) }, &p()).unwrap();
    let listing = list(&zip_path, NameEncoding::Auto, &p()).unwrap();
    assert!(listing.any_encrypted());
    assert!(listing.entries.iter().filter(|e| !e.is_dir).all(|e| e.encrypted));

    let err = extract_all(&listing, &t.path().join("a"), None).unwrap_err();
    assert_eq!(archive_error(&err), Some(&ArchiveError::PasswordRequired));

    let err = extract_all(&listing, &t.path().join("b"), Some("wrong")).unwrap_err();
    assert_eq!(archive_error(&err), Some(&ArchiveError::WrongPassword));

    let dest = t.path().join("c");
    extract_all(&listing, &dest, Some("비밀1234")).unwrap();
    assert_eq!(read(&dest.join("src/hello.txt")), read(&src.join("hello.txt")));

    // The entries really are AES (not ZipCrypto).
    let ar = zip::ZipArchive::new(File::open(&zip_path).unwrap()).unwrap();
    let idx = listing.entries.iter().find(|e| !e.is_dir).unwrap().index;
    // The local header's method field is 99 (AE-x) for WinZip-style AES.
    let start = ar.by_index_data(idx).unwrap().header_start() as usize;
    let raw = read(&zip_path);
    assert_eq!(u16::from_le_bytes([raw[start + 8], raw[start + 9]]), 99);
}

#[test]
fn cp949_names_are_decoded_in_auto_mode() {
    let t = tmp();
    let zip_path = t.path().join("korean.zip");
    let dir = cp949("한글폴더/");
    let file = cp949("한글폴더/문서 파일.txt");
    raw_zip(&zip_path, &[(&dir, b""), (&file, b"annyeong")], 0);

    let listing = list(&zip_path, NameEncoding::Auto, &p()).unwrap();
    let names: Vec<_> = listing.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(names, vec!["한글폴더", "한글폴더/문서 파일.txt"]);
    assert!(listing.entries[0].is_dir);

    // Forcing CP437 gives the classic mojibake instead.
    let l437 = list(&zip_path, NameEncoding::Cp437, &p()).unwrap();
    assert_ne!(l437.entries[1].path, "한글폴더/문서 파일.txt");
    // Forcing CP949 explicitly also works.
    let l949 = list(&zip_path, NameEncoding::Cp949, &p()).unwrap();
    assert_eq!(l949.entries[1].path, "한글폴더/문서 파일.txt");

    let dest = t.path().join("out");
    extract_all(&listing, &dest, None).unwrap();
    assert_eq!(read(&dest.join("한글폴더/문서 파일.txt")), b"annyeong");
}

#[test]
fn utf8_flagged_names_win_over_cp949() {
    let t = tmp();
    let zip_path = t.path().join("utf8.zip");
    let name = "사진/여름.jpg".as_bytes();
    raw_zip(&zip_path, &[(name, b"x")], 0x0800);
    let listing = list(&zip_path, NameEncoding::Auto, &p()).unwrap();
    assert_eq!(listing.entries[0].path, "사진/여름.jpg");
}

#[test]
fn decode_name_rules() {
    assert_eq!(decode_name(b"plain.txt", false, NameEncoding::Auto), "plain.txt");
    assert_eq!(decode_name(&cp949("가나다.txt"), false, NameEncoding::Auto), "가나다.txt");
    assert_eq!(decode_name("가나다".as_bytes(), false, NameEncoding::Auto), "가나다");
    assert_eq!(decode_name(&[0x80, 0x81], false, NameEncoding::Cp437), "Çü");
    // CP949 extension characters (outside plain EUC-KR) decode too: "똠" is 0x8C63 in CP949.
    assert_eq!(decode_name(&[0x8C, 0x63], false, NameEncoding::Cp949), "똠");
}

#[test]
fn zip_slip_entries_are_skipped() {
    let t = tmp();
    let zip_path = t.path().join("evil.zip");
    raw_zip(&zip_path, &[(b"../evil.txt", b"bad"), (b"ok/fine.txt", b"good"), (b"/abs.txt", b"abs")], 0);
    let listing = list(&zip_path, NameEncoding::Auto, &p()).unwrap();
    let dest = t.path().join("inner/dest");
    let rep = extract_all(&listing, &dest, None).unwrap();
    assert_eq!(rep.skipped_unsafe, 1);
    assert!(!t.path().join("inner/evil.txt").exists());
    assert_eq!(read(&dest.join("ok/fine.txt")), b"good");
    // A leading slash is made relative, never absolute.
    assert_eq!(read(&dest.join("abs.txt")), b"abs");
    assert!(safe_relative_path("a/../../b").is_none());
    assert!(safe_relative_path("C:/Windows/x").is_none());
    assert_eq!(safe_relative_path("a\\b/c").unwrap(), PathBuf::from("a").join("b").join("c"));
}

#[test]
fn selected_extract_strips_current_folder() {
    let t = tmp();
    let src = make_tree(t.path());
    let zip_path = t.path().join("sel.zip");
    create_zip(&zip_path, &[src], &ZipOptions::default(), &p()).unwrap();
    let listing = list(&zip_path, NameEncoding::Auto, &p()).unwrap();
    let dest = t.path().join("d");
    let rep = extract(
        &listing,
        &ExtractOptions {
            dest: dest.clone(),
            selection: Some(vec!["src/문서/깊은".into()]),
            strip_prefix: "src/문서".into(),
            password: None,
            overwrite: Overwrite::Always,
        },
        &p(),
    )
    .unwrap();
    assert_eq!(rep.files, 1);
    assert!(dest.join("깊은/data.bin").exists());
    assert!(!dest.join("hello.txt").exists());
}

#[test]
fn overwrite_skip_and_rename() {
    let t = tmp();
    let f = t.path().join("a.txt");
    fs::write(&f, "new").unwrap();
    let zip_path = t.path().join("o.zip");
    create_zip(&zip_path, &[f], &ZipOptions::default(), &p()).unwrap();
    let listing = list(&zip_path, NameEncoding::Auto, &p()).unwrap();
    let dest = t.path().join("d");
    fs::create_dir_all(&dest).unwrap();
    fs::write(dest.join("a.txt"), "old").unwrap();
    let mut opts = ExtractOptions {
        dest: dest.clone(),
        selection: None,
        strip_prefix: String::new(),
        password: None,
        overwrite: Overwrite::Skip,
    };
    let rep = extract(&listing, &opts, &p()).unwrap();
    assert_eq!(rep.skipped_existing, 1);
    assert_eq!(read(&dest.join("a.txt")), b"old");
    opts.overwrite = Overwrite::Rename;
    extract(&listing, &opts, &p()).unwrap();
    assert_eq!(read(&dest.join("a (1).txt")), b"new");
    opts.overwrite = Overwrite::Always;
    extract(&listing, &opts, &p()).unwrap();
    assert_eq!(read(&dest.join("a.txt")), b"new");
}

#[test]
fn add_and_delete_in_existing_zip() {
    let t = tmp();
    let src = make_tree(t.path());
    // Start from an archive that has a CP949 name, to prove raw copies keep it.
    let zip_path = t.path().join("edit.zip");
    let kname = cp949("옛날파일.txt");
    raw_zip(&zip_path, &[(&kname, b"legacy")], 0);

    let listing = list(&zip_path, NameEncoding::Auto, &p()).unwrap();
    let added = add_to_zip(&listing, &[src.join("hello.txt"), src.join("문서")], "새폴더", &ZipOptions::default(), &p()).unwrap();
    assert_eq!(added, 3);
    let listing = list(&zip_path, NameEncoding::Auto, &p()).unwrap();
    let names: HashSet<_> = listing.entries.iter().map(|e| e.path.clone()).collect();
    assert!(names.contains("옛날파일.txt"), "{names:?}");
    assert!(names.contains("새폴더/hello.txt"));
    assert!(names.contains("새폴더/문서/깊은/data.bin"));

    // Re-adding the same file replaces it rather than duplicating it.
    add_to_zip(&listing, &[src.join("hello.txt")], "새폴더", &ZipOptions::default(), &p()).unwrap();
    let listing = list(&zip_path, NameEncoding::Auto, &p()).unwrap();
    assert_eq!(listing.entries.iter().filter(|e| e.path == "새폴더/hello.txt").count(), 1);

    let removed = delete_from_zip(&listing, &["새폴더/문서".into()], &p()).unwrap();
    assert!(removed >= 3);
    let listing = list(&zip_path, NameEncoding::Auto, &p()).unwrap();
    assert!(listing.entries.iter().all(|e| !e.path.starts_with("새폴더/문서")));
    let dest = t.path().join("x");
    extract_all(&listing, &dest, None).unwrap();
    assert_eq!(read(&dest.join("옛날파일.txt")), b"legacy");
    assert_eq!(read(&dest.join("새폴더/hello.txt")), read(&src.join("hello.txt")));
}

#[test]
fn add_with_password_to_existing_zip() {
    let t = tmp();
    let src = make_tree(t.path());
    let zip_path = t.path().join("p.zip");
    create_zip(&zip_path, &[src.join("hello.txt")], &ZipOptions::default(), &p()).unwrap();
    let listing = list(&zip_path, NameEncoding::Auto, &p()).unwrap();
    add_to_zip(&listing, &[src.join("문서/보고서.txt")], "", &ZipOptions { level: 6, password: Some("pw".into()) }, &p()).unwrap();
    let listing = list(&zip_path, NameEncoding::Auto, &p()).unwrap();
    let enc: Vec<_> = listing.entries.iter().filter(|e| e.encrypted).map(|e| e.path.as_str()).collect();
    assert_eq!(enc, vec!["보고서.txt"]);
    let dest = t.path().join("d");
    extract_all(&listing, &dest, Some("pw")).unwrap();
    assert_eq!(read(&dest.join("보고서.txt")), read(&src.join("문서/보고서.txt")));
}

fn make_tar_bytes(src: &Path) -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    b.append_dir_all("src", src).unwrap();
    b.into_inner().unwrap()
}

fn check_tar_like(path: &Path, kind: ArchiveKind, src: &Path) {
    let listing = list(path, NameEncoding::Auto, &p()).unwrap();
    assert_eq!(listing.kind, kind, "{}", path.display());
    assert_eq!(listing.file_count(), 3);
    assert!(listing.entries.iter().any(|e| e.path == "src/문서/깊은/data.bin" && e.size == 5000));
    let dest = path.with_extension("out");
    extract_all(&listing, &dest, None).unwrap();
    assert_eq!(read(&dest.join("src/문서/보고서.txt")), read(&src.join("문서/보고서.txt")));
    assert_eq!(read(&dest.join("src/hello.txt")), read(&src.join("hello.txt")));
}

#[test]
fn tar_and_compressed_tars() {
    let t = tmp();
    let src = make_tree(t.path());
    let tar_bytes = make_tar_bytes(&src);

    let tar_path = t.path().join("a.tar");
    fs::write(&tar_path, &tar_bytes).unwrap();
    check_tar_like(&tar_path, ArchiveKind::Tar, &src);

    let gz_path = t.path().join("a.tar.gz");
    let mut gz = flate2::write::GzEncoder::new(File::create(&gz_path).unwrap(), flate2::Compression::default());
    gz.write_all(&tar_bytes).unwrap();
    gz.finish().unwrap();
    check_tar_like(&gz_path, ArchiveKind::TarGz, &src);

    // Detection is by content, not by extension.
    let tgz = t.path().join("renamed.bin");
    fs::copy(&gz_path, &tgz).unwrap();
    assert_eq!(detect_kind(&tgz).unwrap(), ArchiveKind::TarGz);

    let bz_path = t.path().join("a.tar.bz2");
    let mut bz = bzip2::write::BzEncoder::new(File::create(&bz_path).unwrap(), bzip2::Compression::default());
    bz.write_all(&tar_bytes).unwrap();
    bz.finish().unwrap();
    check_tar_like(&bz_path, ArchiveKind::TarBz2, &src);

    let xz_path = t.path().join("a.tar.xz");
    let mut xz = lzma_rust2::XzWriter::new(File::create(&xz_path).unwrap(), lzma_rust2::XzOptions::with_preset(6)).unwrap();
    xz.write_all(&tar_bytes).unwrap();
    xz.finish().unwrap();
    check_tar_like(&xz_path, ArchiveKind::TarXz, &src);
}

#[test]
fn single_gz_file() {
    let t = tmp();
    let data = "로그 한 줄\n".repeat(1000);
    let gz_path = t.path().join("server.log.gz");
    let mut gz = flate2::GzBuilder::new()
        .filename("server.log")
        .mtime(1_700_000_000)
        .write(File::create(&gz_path).unwrap(), flate2::Compression::best());
    gz.write_all(data.as_bytes()).unwrap();
    gz.finish().unwrap();

    let listing = list(&gz_path, NameEncoding::Auto, &p()).unwrap();
    assert_eq!(listing.kind, ArchiveKind::Gz);
    assert_eq!(listing.entries.len(), 1);
    let e = &listing.entries[0];
    assert_eq!(e.path, "server.log");
    assert_eq!(e.size, data.len() as u64);
    assert!(e.modified.is_some());
    let dest = t.path().join("d");
    extract_all(&listing, &dest, None).unwrap();
    assert_eq!(read(&dest.join("server.log")), data.as_bytes());
}

#[test]
fn sevenz_extract() {
    let t = tmp();
    let src = make_tree(t.path());
    let path = t.path().join("a.7z");
    sevenz_rust2::compress_to_path(&src, &path).unwrap();
    let listing = list(&path, NameEncoding::Auto, &p()).unwrap();
    assert_eq!(listing.kind, ArchiveKind::SevenZ);
    assert_eq!(listing.file_count(), 3);
    let data_entry = listing.entries.iter().find(|e| e.path.ends_with("깊은/data.bin")).unwrap();
    assert_eq!(data_entry.size, 5000);
    let dest = t.path().join("d");
    let rep = extract_all(&listing, &dest, None).unwrap();
    assert_eq!(rep.files, 3);
    let got = rep.written.iter().find(|w| w.ends_with("보고서.txt")).unwrap();
    assert_eq!(read(got), read(&src.join("문서/보고서.txt")));
}

#[test]
fn sevenz_with_password() {
    let t = tmp();
    let src = make_tree(t.path());
    let path = t.path().join("enc.7z");
    sevenz_rust2::compress_to_path_encrypted(&src, &path, sevenz_rust2::Password::new("암호")).unwrap();
    // Header may be encrypted: listing then needs the password.
    match list(&path, NameEncoding::Auto, &p()) {
        Ok(listing) => {
            let err = extract_all(&listing, &t.path().join("x"), None).unwrap_err();
            assert!(matches!(archive_error(&err), Some(ArchiveError::PasswordRequired | ArchiveError::WrongPassword)), "{err}");
            let dest = t.path().join("y");
            extract_all(&listing, &dest, Some("암호")).unwrap();
            assert!(dest.join("문서/보고서.txt").exists() || dest.join("src/문서/보고서.txt").exists());
        }
        Err(err) => {
            assert_eq!(archive_error(&err), Some(&ArchiveError::PasswordRequired));
            let listing = list_with_password(&path, NameEncoding::Auto, Some("암호"), &p()).unwrap();
            assert_eq!(listing.file_count(), 3);
        }
    }
}

#[test]
fn folder_navigation_with_implied_dirs() {
    let entries: Vec<Entry> = ["a/b/c.txt", "a/d.txt", "top.txt", "e/"]
        .iter()
        .enumerate()
        .map(|(i, pth)| Entry {
            index: i,
            path: normalize_entry_path(pth),
            is_dir: pth.ends_with('/'),
            size: 10,
            packed: Some(5),
            modified: None,
            encrypted: false,
            method: String::new(),
        })
        .collect();
    let root = children(&entries, "");
    let names: Vec<_> = root.iter().map(|c| (c.name.as_str(), c.is_dir)).collect();
    // Folders first, then files.
    assert_eq!(names, vec![("a", true), ("e", true), ("top.txt", false)]);
    assert_eq!(root[0].file_count, 2);
    assert_eq!(root[0].size, 20);
    let a = children(&entries, "a");
    assert_eq!(a.iter().map(|c| c.path.as_str()).collect::<Vec<_>>(), vec!["a/b", "a/d.txt"]);
    assert_eq!(all_folders(&entries), vec!["a", "a/b", "e"]);
    assert_eq!(parent_dir("a/b"), "a");
    assert_eq!(parent_dir("a"), "");
}

#[test]
fn cancel_stops_extraction() {
    let t = tmp();
    let src = make_tree(t.path());
    let zip_path = t.path().join("c.zip");
    create_zip(&zip_path, &[src], &ZipOptions::default(), &p()).unwrap();
    let listing = list(&zip_path, NameEncoding::Auto, &p()).unwrap();
    let prog = p();
    prog.cancel.store(true, Ordering::Relaxed);
    let err = extract(
        &listing,
        &ExtractOptions {
            dest: t.path().join("d"),
            selection: None,
            strip_prefix: String::new(),
            password: None,
            overwrite: Overwrite::Always,
        },
        &prog,
    )
    .unwrap_err();
    assert_eq!(archive_error(&err), Some(&ArchiveError::Cancelled));
}

#[test]
fn unknown_files_are_rejected_and_sizes_format() {
    let t = tmp();
    let f = t.path().join("note.txt");
    fs::write(&f, "not an archive").unwrap();
    assert!(detect_kind(&f).is_err());
    assert!(!is_archive_file(&f));
    assert_eq!(format_size(512), "512 B");
    assert_eq!(format_size(1536), "1.5 KB");
    assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
    assert_eq!(ratio(200, Some(50)), Some(25.0));
}
