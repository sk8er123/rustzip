//! Every format RustZip creates must reopen and extract to the same tree, and every
//! format it only reads is checked against archives made by other programs.

use super::*;
use std::collections::BTreeMap;

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(rel)
}

fn tree(root: &Path) -> PathBuf {
    let src = root.join("자료");
    fs::create_dir_all(src.join("문서/깊은")).unwrap();
    fs::create_dir_all(src.join("빈 폴더")).unwrap();
    fs::write(src.join("hello.txt"), "hello world\n".repeat(500)).unwrap();
    fs::write(src.join("문서/보고서.txt"), "한글 내용입니다.".repeat(300)).unwrap();
    let noise: Vec<u8> = (0..300_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    fs::write(src.join("문서/깊은/data.bin"), noise).unwrap();
    src
}

/// relative path → contents (None for folders)
fn snapshot(dir: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, Option<Vec<u8>>>) {
        for e in fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
            if p.is_dir() {
                out.insert(rel, None);
                walk(base, &p, out);
            } else {
                out.insert(rel, Some(fs::read(&p).unwrap()));
            }
        }
    }
    let mut m = BTreeMap::new();
    walk(dir, dir, &mut m);
    m
}

fn extract_all(l: &Listing, dest: &Path, password: Option<&str>) -> Result<ExtractReport> {
    let opts = ExtractOptions { dest: dest.into(), selection: None, strip_prefix: String::new(), password: password.map(Into::into), overwrite: Overwrite::Always };
    extract(l, &opts, &Progress::default())
}

fn open(p: &Path) -> Result<Listing> {
    list(p, NameEncoding::Auto, &Progress::default())
}

fn open_pw(p: &Path, pw: &str) -> Result<Listing> {
    list_with_password(p, NameEncoding::Auto, Some(pw), &Progress::default())
}

fn is(err: &anyhow::Error, want: ArchiveError) -> bool {
    archive_error(err) == Some(&want)
}

fn roundtrip(kind: ArchiveKind, opts: &PackOptions) {
    if let Err(e) = std::panic::catch_unwind(|| roundtrip_inner(kind, opts)) {
        let msg = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
        FAILURES.with(|f| f.borrow_mut().push(msg.lines().next().unwrap_or("").to_string()));
    }
}

thread_local! {
    static FAILURES: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn assert_no_failures() {
    let f = FAILURES.with(|f| std::mem::take(&mut *f.borrow_mut()));
    assert!(f.is_empty(), "{} failure(s):\n{}", f.len(), f.join("\n"));
}

fn roundtrip_inner(kind: ArchiveKind, opts: &PackOptions) {
    let t = tmp();
    let src = tree(t.path());
    let single = src.join("문서/보고서.txt");
    let sources = if kind.single_file_only() { vec![single.clone()] } else { vec![src.clone()] };
    // Single-file archives keep the file's own name: 보고서.txt → 보고서.txt.gz
    let dest = if kind.single_file_only() { t.path().join(format!("보고서.txt.{}", kind.extension())) } else { t.path().join(format!("결과.{}", kind.extension())) };
    let what = format!("{} {:?} level {}", kind.label(), opts.method, opts.level);
    create(&dest, &sources, kind, opts, &Progress::default()).unwrap_or_else(|e| panic!("{what}: {e:#}"));
    let l = match &opts.password {
        Some(pw) => open_pw(&dest, pw),
        None => open(&dest),
    }
    .unwrap_or_else(|e| panic!("{what}: {e:#}"));
    assert_eq!(l.kind, kind, "{what}: detected as {}", l.kind.label());
    let out = t.path().join("풀기");
    extract_all(&l, &out, opts.password.as_deref()).unwrap_or_else(|e| panic!("{what}: {e:#}"));
    if kind.single_file_only() {
        let got = snapshot(&out);
        assert_eq!(got.len(), 1, "{what}");
        assert_eq!(got.get("보고서.txt"), Some(&Some(fs::read(&single).unwrap())), "{what}");
    } else {
        assert_eq!(snapshot(&out.join("자료")), snapshot(&src), "{what}");
    }
}

#[test]
fn every_creatable_format_and_method_roundtrips() {
    for kind in ArchiveKind::creatable() {
        let methods: Vec<Method> = if kind.methods().is_empty() { vec![Method::Deflate] } else { kind.methods().to_vec() };
        for method in methods {
            for level in [1, 9] {
                roundtrip(kind, &PackOptions { method, level, password: None });
            }
        }
    }
    assert_no_failures();
}

#[test]
fn level_zero_stores_in_every_format() {
    for kind in ArchiveKind::creatable() {
        roundtrip(kind, &PackOptions { level: 0, ..Default::default() });
    }
    assert_no_failures();
}

#[test]
fn passwords_for_zip_and_7z_in_every_method() {
    for kind in [ArchiveKind::ZIP, ArchiveKind::SEVENZ] {
        for method in kind.methods_for(true) {
            roundtrip(kind, &PackOptions { method, level: 5, password: Some("비밀 pw".into()) });
        }
        assert_no_failures();
        let t = tmp();
        let src = tree(t.path());
        let dest = t.path().join(format!("a.{}", kind.extension()));
        create(&dest, &[src], kind, &PackOptions { password: Some("right".into()), method: kind.methods()[0], level: 5 }, &Progress::default()).unwrap();
        if kind == ArchiveKind::SEVENZ {
            // The file list itself is encrypted.
            assert!(is(&open(&dest).unwrap_err(), ArchiveError::PasswordRequired));
            assert!(is(&open_pw(&dest, "wrong").unwrap_err(), ArchiveError::WrongPassword));
        } else {
            let l = open(&dest).unwrap();
            assert!(is(&extract_all(&l, &t.path().join("x"), None).unwrap_err(), ArchiveError::PasswordRequired));
            assert!(is(&extract_all(&l, &t.path().join("y"), Some("wrong")).unwrap_err(), ArchiveError::WrongPassword));
        }
    }
}

#[test]
fn single_file_formats_refuse_folders_and_many_files() {
    let t = tmp();
    let src = tree(t.path());
    let kind = ArchiveKind::new(Container::Raw, Filter::Zstd);
    assert!(create(&t.path().join("a.zst"), std::slice::from_ref(&src), kind, &PackOptions::default(), &Progress::default()).is_err());
    assert!(create(&t.path().join("b.zst"), &[src.join("hello.txt"), src.join("문서/보고서.txt")], kind, &PackOptions::default(), &Progress::default()).is_err());
    assert!(!t.path().join("a.zst").exists() && !t.path().join("b.zst").exists());
}

#[test]
fn rar_archives_with_and_without_passwords() {
    let l = open(&fixture("rar/crypted.rar")).unwrap();
    assert_eq!(l.kind, ArchiveKind::plain(Container::Rar));
    assert_eq!(l.entries[0].path, ".gitignore");
    assert!(l.entries[0].encrypted);
    let t = tmp();
    assert!(is(&extract_all(&l, &t.path().join("a"), None).unwrap_err(), ArchiveError::PasswordRequired));
    assert!(is(&extract_all(&l, &t.path().join("b"), Some("nope")).unwrap_err(), ArchiveError::WrongPassword));
    extract_all(&l, &t.path().join("c"), Some("unrar")).unwrap();
    assert_eq!(fs::read_to_string(t.path().join("c/.gitignore")).unwrap(), "target\nCargo.lock\n");

    // Encrypted file list: even listing needs the password.
    let hp = fixture("rar/comment-hpw-password.rar");
    assert!(is(&open(&hp).unwrap_err(), ArchiveError::PasswordRequired));
    let l = open_pw(&hp, "password").unwrap();
    extract_all(&l, &t.path().join("d"), Some("password")).unwrap();
    assert_eq!(fs::read_to_string(t.path().join("d/.gitignore")).unwrap(), "target\nCargo.lock\n");

    for name in ["solid.rar", "unicode.rar"] {
        let l = open(&fixture(&format!("rar/{name}"))).unwrap();
        assert!(l.file_count() > 0, "{name}");
        let rep = extract_all(&l, &t.path().join(name), None).unwrap();
        assert_eq!(rep.files, l.file_count(), "{name}");
        assert_eq!(rep.bytes, l.total_size(), "{name}");
    }
}

#[test]
fn arj_all_methods() {
    let license = fs::read(fixture("arj/LICENSE")).unwrap();
    for m in ["stored", "method1", "method2", "method3", "method4"] {
        let l = open(&fixture(&format!("arj/{m}.arj"))).unwrap();
        assert_eq!(l.kind, ArchiveKind::plain(Container::Arj));
        assert_eq!(l.entries.len(), 1, "{m}");
        let t = tmp();
        extract_all(&l, t.path(), None).unwrap();
        assert_eq!(fs::read(t.path().join("LICENSE")).unwrap(), license, "{m}");
    }
}

#[test]
fn lzh_header_levels_and_methods() {
    for name in ["explzh_723_h0_lh0.lzh", "explzh_723_h0_lh5.lzh", "explzh_723_h1_lh6.lzh", "explzh_723_h2_lh7.lzh", "explzh_723_h2_subdir.lzh", "lengths_lh1-64k.lzh", "larc_lz5.lzs"] {
        let l = open(&fixture(&format!("lzh/{name}"))).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        assert_eq!(l.kind, ArchiveKind::plain(Container::Lzh), "{name}");
        assert!(l.file_count() > 0, "{name}");
        let t = tmp();
        // The reader checks each file's CRC-16, so success means the bytes are right.
        let rep = extract_all(&l, t.path(), None).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        assert_eq!(rep.files, l.file_count(), "{name}");
        assert_eq!(rep.bytes, l.total_size(), "{name}");
    }
    let l = open(&fixture("lzh/explzh_723_h2_subdir.lzh")).unwrap();
    assert!(l.entries.iter().any(|e| e.path.contains('/')), "{:?}", l.entries.iter().map(|e| &e.path).collect::<Vec<_>>());
}

#[test]
fn cab_stored_and_mszip() {
    let t = tmp();
    let text = "cabinet 내용 ".repeat(5000);
    for ctype in [cab::CompressionType::None, cab::CompressionType::MsZip] {
        let path = t.path().join("a.cab");
        let mut b = cab::CabinetBuilder::new();
        let folder = b.add_folder(ctype);
        folder.add_file("dir\\one.txt");
        folder.add_file("two.bin");
        let mut w = b.build(File::create(&path).unwrap()).unwrap();
        while let Some(mut f) = w.next_file().unwrap() {
            let data: Vec<u8> = if f.file_name().ends_with("txt") { text.as_bytes().to_vec() } else { (0..70_000u32).map(|i| (i % 251) as u8).collect() };
            f.write_all(&data).unwrap();
        }
        w.finish().unwrap();
        let l = open(&path).unwrap();
        assert_eq!(l.kind, ArchiveKind::plain(Container::Cab));
        let out = t.path().join(format!("{ctype:?}"));
        extract_all(&l, &out, None).unwrap();
        assert_eq!(fs::read_to_string(out.join("dir/one.txt")).unwrap(), text);
        assert_eq!(fs::read(out.join("two.bin")).unwrap().len(), 70_000);
    }
}

#[test]
fn compress_z_detected_and_decoded() {
    let t = tmp();
    let data = "compress .Z 데이터 ".repeat(20_000);
    let z = super::lzw::tests::compress(data.as_bytes(), 16);
    fs::write(t.path().join("a.txt.Z"), &z).unwrap();
    let l = open(&t.path().join("a.txt.Z")).unwrap();
    assert_eq!(l.kind, ArchiveKind::new(Container::Raw, Filter::Compress));
    assert_eq!(l.entries[0].path, "a.txt");
    assert_eq!(l.entries[0].size, data.len() as u64);
    extract_all(&l, &t.path().join("o"), None).unwrap();
    assert_eq!(fs::read_to_string(t.path().join("o/a.txt")).unwrap(), data);
}

#[cfg(unix)]
mod with_system_tools {
    use super::*;
    use std::process::Command;

    fn have(tool: &str) -> bool {
        let ok = Command::new("sh").arg("-c").arg(format!("command -v {tool}")).output().is_ok_and(|o| o.status.success());
        if !ok {
            eprintln!("skipped: {tool} is not installed");
        }
        ok
    }

    fn sh(dir: &Path, script: &str) {
        let o = Command::new("sh").arg("-c").arg(script).current_dir(dir).output().unwrap();
        assert!(o.status.success(), "{script}: {}", String::from_utf8_lossy(&o.stderr));
    }

    fn check(archive: &Path, kind: ArchiveKind, expected_root: &Path) {
        let l = open(archive).unwrap_or_else(|e| panic!("{}: {e:#}", archive.display()));
        assert_eq!(l.kind, kind, "{}", archive.display());
        let t = tmp();
        extract_all(&l, t.path(), None).unwrap();
        let want: BTreeMap<_, _> = snapshot(expected_root).into_iter().filter(|(_, v)| v.is_some()).collect();
        let got: BTreeMap<_, _> = snapshot(&t.path().join("자료")).into_iter().filter(|(_, v)| v.is_some()).collect();
        assert_eq!(got, want, "{}", archive.display());
    }

    #[test]
    fn our_compress_encoder_matches_system_uncompress() {
        if !have("uncompress") {
            return;
        }
        let t = tmp();
        let pattern = "abcabcabd hello 압축 ".as_bytes();
        let data: Vec<u8> = (0..400_000usize).map(|i| pattern[(i * 7 + i / 3) % pattern.len()]).collect();
        fs::write(t.path().join("x.Z"), super::super::lzw::tests::compress(&data, 16)).unwrap();
        sh(t.path(), "uncompress -c x.Z > x.out");
        assert_eq!(fs::read(t.path().join("x.out")).unwrap(), data);
    }

    #[test]
    fn cpio_formats_from_gnu_cpio() {
        if !have("cpio") {
            return;
        }
        let t = tmp();
        let src = tree(t.path());
        for fmt in ["newc", "crc", "odc", "bin"] {
            sh(t.path(), &format!("find 자료 | cpio -o -H {fmt} --quiet > {fmt}.cpio"));
            check(&t.path().join(format!("{fmt}.cpio")), ArchiveKind::plain(Container::Cpio), &src);
        }
        sh(t.path(), "gzip -k newc.cpio");
        check(&t.path().join("newc.cpio.gz"), ArchiveKind::new(Container::Cpio, Filter::Gzip), &src);
    }

    #[test]
    fn ar_from_binutils_with_long_names() {
        if !have("ar") {
            return;
        }
        let t = tmp();
        fs::write(t.path().join("short.o"), b"short").unwrap();
        fs::write(t.path().join("a_rather_long_member_name.txt"), "long name contents".repeat(10)).unwrap();
        sh(t.path(), "ar rc lib.a short.o a_rather_long_member_name.txt");
        let l = open(&t.path().join("lib.a")).unwrap();
        assert_eq!(l.kind, ArchiveKind::plain(Container::Ar));
        let names: Vec<&str> = l.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(names, ["short.o", "a_rather_long_member_name.txt"]);
        let out = t.path().join("o");
        extract_all(&l, &out, None).unwrap();
        assert_eq!(fs::read(out.join("a_rather_long_member_name.txt")).unwrap(), "long name contents".repeat(10).as_bytes());
    }

    #[test]
    fn iso_with_and_without_joliet() {
        if !have("xorriso") {
            return;
        }
        let t = tmp();
        let src = tree(t.path());
        sh(t.path(), "xorriso -as mkisofs -quiet -J -o joliet.iso 자료 2>/dev/null");
        // `-graft-points` keeps the 자료 folder itself as the top entry.
        let l = open(&t.path().join("joliet.iso")).unwrap();
        assert_eq!(l.kind, ArchiveKind::plain(Container::Iso));
        assert!(l.entries.iter().any(|e| e.path == "문서/보고서.txt"), "{:?}", l.entries.iter().map(|e| &e.path).collect::<Vec<_>>());
        let out = t.path().join("j");
        extract_all(&l, &out, None).unwrap();
        assert_eq!(snapshot(&out).into_iter().filter(|(_, v)| v.is_some()).collect::<BTreeMap<_, _>>(), snapshot(&src).into_iter().filter(|(_, v)| v.is_some()).collect());

        sh(t.path(), "mkdir plain && echo plain > plain/readme.txt && xorriso -as mkisofs -quiet -o plain.iso plain 2>/dev/null");
        let l = open(&t.path().join("plain.iso")).unwrap();
        assert_eq!(l.entries.len(), 1);
        assert!(l.entries[0].path.eq_ignore_ascii_case("readme.txt"), "{}", l.entries[0].path);
    }

    #[test]
    fn zstd_cli_archives() {
        if !have("zstd") || !have("tar") {
            return;
        }
        let t = tmp();
        let src = tree(t.path());
        sh(t.path(), "tar -cf - 자료 | zstd -q -o a.tar.zst");
        check(&t.path().join("a.tar.zst"), ArchiveKind::new(Container::Tar, Filter::Zstd), &src);
        sh(t.path(), "zstd -q -19 자료/hello.txt -o hello.txt.zst");
        let l = open(&t.path().join("hello.txt.zst")).unwrap();
        assert_eq!(l.kind, ArchiveKind::new(Container::Raw, Filter::Zstd));
        extract_all(&l, &t.path().join("z"), None).unwrap();
        assert_eq!(fs::read(t.path().join("z/hello.txt")).unwrap(), fs::read(src.join("hello.txt")).unwrap());
    }

    #[test]
    fn our_archives_open_in_system_tools() {
        if !have("tar") || !have("zstd") {
            return;
        }
        let t = tmp();
        let src = tree(t.path());
        for (filter, flag) in [(Filter::Gzip, "-z"), (Filter::Bzip2, "-j"), (Filter::Xz, "-J"), (Filter::Zstd, "--zstd")] {
            let kind = ArchiveKind::new(Container::Tar, filter);
            let a = t.path().join(format!("rz.{}", kind.extension()));
            create(&a, std::slice::from_ref(&src), kind, &PackOptions::default(), &Progress::default()).unwrap();
            let out = t.path().join(format!("out-{}", filter.ext()));
            fs::create_dir_all(&out).unwrap();
            sh(t.path(), &format!("tar {flag} -xf {} -C {}", a.display(), out.display()));
            assert_eq!(snapshot(&out.join("자료")), snapshot(&src), "{}", kind.label());
        }
    }
}

#[test]
fn cancelled_listing_stops_instead_of_retrying() {
    let t = tmp();
    let src = tree(t.path());
    for kind in [ArchiveKind::new(Container::Tar, Filter::Gzip), ArchiveKind::new(Container::Raw, Filter::Zstd)] {
        let source = if kind.single_file_only() { src.join("hello.txt") } else { src.clone() };
        let a = t.path().join(format!("c.{}", kind.extension()));
        create(&a, &[source], kind, &PackOptions::default(), &Progress::default()).unwrap();
        let p = Progress::default();
        p.cancel.store(true, Ordering::Relaxed);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(list(&a, NameEncoding::Auto, &p).map(|_| ()));
        });
        let res = rx.recv_timeout(std::time::Duration::from_secs(10)).expect("listing hung after cancel");
        assert!(is(&res.unwrap_err(), ArchiveError::Cancelled), "{}", kind.label());
    }
}

/// ALZ and EGG through the same entry points the window uses: detect, list, extract.
#[test]
fn every_egg_and_alz_fixture_opens_and_extracts_end_to_end() {
    let dir = fixture("egg");
    let mut names: Vec<String> = fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    let mut checked = 0;
    for name in names {
        let path = dir.join(&name);
        if name == "number.vol2.egg" {
            continue; // second half of number.vol1.egg
        }
        let l = open(&path).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        let want = if name.ends_with(".alz") { Container::Alz } else { Container::Egg };
        assert_eq!(l.kind, ArchiveKind::plain(want), "{name}");
        let pw = l.any_encrypted().then_some("password12345!");
        if pw.is_some() {
            let t = tmp();
            assert!(is(&extract_all(&l, t.path(), None).unwrap_err(), ArchiveError::PasswordRequired), "{name}");
        }
        let t = tmp();
        let rep = extract_all(&l, t.path(), pw).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        assert_eq!(rep.files, l.file_count(), "{name}");
        assert_eq!(rep.bytes, l.total_size(), "{name}");
        checked += 1;
    }
    assert_eq!(checked, 34);
    let l = open(&fixture("egg/small_kor.egg")).unwrap();
    assert!(l.entries.iter().any(|e| e.path == "작은.txt"));
    let l = open(&fixture("egg/azomulti.egg")).unwrap();
    assert!(l.entries.iter().all(|e| e.is_dir || e.method == "AZO"), "{:?}", l.entries.iter().map(|e| &e.method).collect::<Vec<_>>());
}
