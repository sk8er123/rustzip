//! ALZ and EGG walkers against the EggDotNet fixtures (expected sizes, CRC-32s and
//! SHA-256s from EggDotNet.Tests/UnitTest1.cs), plus synthesized ALZ archives for
//! what no fixture has: split volumes, ZipCrypto and ALZ's bzip2 variant.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use sha2::{Digest, Sha256};

use super::super::codec::alzbzip2::to_alz_framing;
use super::super::walk::{self, Item, Source, Visitor};
use super::super::{archive_error, ArchiveError, ExtractOptions, ExtractReport, Filter, NameEncoding, Overwrite, Planner, Progress};
use super::{alz, egg};

const PASSWORD: &str = "password12345!";

/// EggDotNet's TestFileInfos: name, size, CRC-32 as its test prints the little-endian bytes, SHA-256.
const REFERENCE: &[(&str, u64, &str, &str)] = &[
    ("lorem_ipsum_short.txt", 525, "3AAE00F7", "ACFE59AFCFFCBE68A4DEE454D11F5BF78DFDEE32C77A03C1A50CF384418FE38F"),
    ("lorem_ipsum_long.pdf", 66_431, "798BEF9F", "4B3E49290B2399C2AB63886B0585B819AE7C6D9FA947F1B3DB53BD9DA62A2B10"),
    ("lorem_ipsum_long.tif", 1_195_080, "90D70968", "162D8B99FD7E695BD3E8C790493C93B649A76996F4E129C818A366614977EC67"),
    ("lorem_ipsum_medium.txt", 3_950, "AE4BAE1D", "5DEA92AB15A3A73AF46B0AFBF793C109B576E35C62CE9DB703E7F4088F2E8A07"),
    ("lorem_ipsum_long.txt", 15_238, "EE3EDF61", "D63CFF6C64AB12F1C08A298F7B3A0C27F19A9822D2C6C7156D676D62265DB46E"),
    ("lorem_ipsum.zip", 1_242_971, "32AE3E83", "F7E4474594F6B6C8EB4A38A86C5391761AC8C8B8F8CFE2D35101A1B43B467BCC"),
];

const SIX: &[(&str, u64)] = &[
    ("lorem_ipsum_long.pdf", 66_431),
    ("lorem_ipsum_long.tif", 1_195_080),
    ("lorem_ipsum_long.txt", 15_238),
    ("lorem_ipsum_medium.txt", 3_950),
    ("lorem_ipsum_short.txt", 525),
    ("lorem_ipsum.zip", 1_242_971),
];

/// Every fixture with its entries in archive order; a trailing '/' marks a folder.
fn fixtures() -> Vec<(&'static str, Vec<(&'static str, u64)>)> {
    let six = |order: [usize; 6]| order.iter().map(|&i| SIX[i]).collect::<Vec<_>>();
    let one = |name, size| vec![(name, size)];
    vec![
        ("azomulti.egg", vec![("lorem_ipsum_long_tif.sys", 1_195_080), ("lorem_ipsum_medium_txt.sys", 3_950), ("lorem_ipsum_short_txt.sys", 525)]),
        ("azonumbers.egg", one("numbers_txt.sys", 6_999_999)),
        ("azoshort_txt.egg", one("azoshorttxt.sys", 128)),
        ("basic_deflate_folders.egg", vec![("backgrounds/bliss.jpg", 745_473), ("lorems/Lorem_ipsum.pdf", 70_011), ("lorems/lorem_ipsum.txt", 34_446)]),
        ("default_lzma_pri_compress_ratio.egg", six([2, 3, 4, 5, 0, 1])),
        ("defaults.alz", six([1, 2, 3, 4, 5, 0])),
        ("defaults.egg", six([0, 1, 2, 3, 4, 5])),
        ("defaults_bz_optimized.egg", six([2, 3, 4, 5, 0, 1])),
        ("defaults_nocompress.egg", six([3, 4, 5, 0, 1, 2])),
        ("defaults_normal.egg", six([4, 5, 0, 1, 2, 3])),
        ("directories.egg", one("dir1/dir2/dir3/test.txt", 18)),
        (
            "extattr.egg",
            vec![("Somehiddenfile.txt", 18), ("ReadonlyFile.txt", 18), ("test.txt", 20), ("Some folder/", 0), ("testlink", 20)],
        ),
        ("globalcomment.egg", one("test.txt", 30)),
        ("globalencrypt.egg", one("test.txt", 30)),
        ("lorem_long_aes128.egg", one("lorem_ipsum_long.txt", 15_238)),
        ("lorem_long_aes256.egg", one("lorem_ipsum_long.txt", 15_238)),
        ("lorem_long_store_lea128.egg", one("lorem_ipsum_long.txt", 15_238)),
        ("lorem_long_store_lea256.egg", one("lorem_ipsum_long.txt", 15_238)),
        ("lorem_long_zipEnc.egg", one("lorem_ipsum_long.txt", 15_238)),
        ("lorem_short_readonlyhidden.egg", vec![("lorem_ipsum_short_readonly.txt", 525), ("lorem_ipsum_short_hidden.txt", 525)]),
        ("lzma_simple.egg", vec![("test.txt", 15), ("lorem_ipsum.txt", 34_446)]),
        ("number.vol1.egg", one("number.txt", 6_999_999)),
        ("number.vol2.egg", one("number.txt", 6_999_999)),
        ("posix.egg", six([5, 0, 1, 2, 3, 4])),
        ("posix_small.egg", one("posix.txt", 17)),
        ("small_kor.egg", one("작은.txt", 13)),
        ("small_shift.egg", one("小さい.txt", 13)),
        ("solid.egg", vec![("s/lorem_ipsum_short.txt", 525), ("s/lorem_ipsum_short2.txt", 525)]),
        (
            "solid_defaults.egg",
            SIX.iter().enumerate().map(|(i, _)| [5, 0, 1, 2, 3, 4][i]).map(|i| (leak(format!("s2/{}", SIX[i].0)), SIX[i].1)).collect(),
        ),
        ("test128.egg", one("test.txt", 30)),
        ("test256.egg", one("test.txt", 30)),
        ("test_lea128_store.egg", one("test.txt", 15)),
        ("test_lea256_store.egg", one("test.txt", 15)),
        ("testzipenc.egg", one("test.txt", 30)),
        ("zeros.egg", one("zeros.txt", 197_540_460)),
    ]
}

const ENCRYPTED: &[(&str, &str)] = &[
    ("lorem_long_zipEnc.egg", "ZipCrypto"),
    ("testzipenc.egg", "ZipCrypto"),
    ("lorem_long_aes128.egg", "AES-128"),
    ("test128.egg", "AES-128"),
    ("lorem_long_aes256.egg", "AES-256"),
    ("test256.egg", "AES-256"),
    ("globalencrypt.egg", "AES-256"),
    ("lorem_long_store_lea128.egg", "LEA-128"),
    ("test_lea128_store.egg", "LEA-128"),
    ("lorem_long_store_lea256.egg", "LEA-256"),
    ("test_lea256_store.egg", "LEA-256"),
];

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/egg").join(name)
}

fn walker_for(path: &Path) -> walk::Walker {
    if path.extension().is_some_and(|e| e == "alz") {
        alz::walk
    } else {
        egg::walk
    }
}

#[derive(Debug, Default)]
struct Contents {
    len: u64,
    crc: u32,
    sha: String,
    /// Kept only for small entries.
    bytes: Vec<u8>,
}

#[derive(Debug)]
struct Seen {
    item: Item,
    contents: Option<Contents>,
}

/// Reads every file entry fully, hashing it as it streams.
struct Collect(Vec<Seen>);

impl Visitor for Collect {
    fn entry(&mut self, item: &Item) -> Result<bool> {
        self.0.push(Seen { item: item.clone(), contents: None });
        Ok(!item.is_dir)
    }
    fn data(&mut self, _: &Item, data: &mut dyn Read) -> Result<()> {
        let mut c = Contents::default();
        let (mut crc, mut sha) = (crc32fast::Hasher::new(), Sha256::new());
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = data.read(&mut buf)?;
            if n == 0 {
                break;
            }
            c.len += n as u64;
            crc.update(&buf[..n]);
            sha.update(&buf[..n]);
            if c.len <= 1 << 16 {
                c.bytes.extend_from_slice(&buf[..n]);
            }
        }
        c.crc = crc.finalize();
        c.sha = sha.finalize().iter().map(|b| format!("{b:02X}")).collect();
        self.0.last_mut().expect("entry came first").contents = Some(c);
        Ok(())
    }
}

fn read_archive(path: &Path, password: Option<&str>) -> Result<Vec<Seen>> {
    let progress = Progress::default();
    let src = Source { path, filter: Filter::None, encoding: NameEncoding::Auto, password, input: &progress };
    let mut v = Collect(Vec::new());
    walker_for(path)(&src, &mut v)?;
    Ok(v.0)
}

fn read_fixture(name: &str, password: Option<&str>) -> Result<Vec<Seen>> {
    read_archive(&fixture(name), password)
}

fn list(path: &Path, password: Option<&str>) -> Result<Vec<super::super::Entry>> {
    let progress = Progress::default();
    let src = Source { path, filter: Filter::None, encoding: NameEncoding::Auto, password, input: &progress };
    walk::list(walker_for(path), &src)
}

fn extract(path: &Path, dest: &Path, selection: Option<Vec<String>>, password: Option<&str>) -> Result<ExtractReport> {
    let opts = ExtractOptions {
        dest: dest.to_path_buf(),
        selection,
        strip_prefix: String::new(),
        password: password.map(str::to_string),
        overwrite: Overwrite::Always,
    };
    let progress = Progress::default();
    let src = Source { path, filter: Filter::None, encoding: NameEncoding::Auto, password, input: &progress };
    let mut planner = Planner { opts: &opts, report: ExtractReport::default() };
    walk::extract(walker_for(path), &src, &mut planner, &progress)?;
    Ok(planner.report)
}

fn sha_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02X}")).collect()
}

/// Fixture entries that are reference files under another name.
const ALIASES: &[(&str, &str)] = &[
    ("lorem_ipsum_long_tif.sys", "lorem_ipsum_long.tif"),
    ("lorem_ipsum_medium_txt.sys", "lorem_ipsum_medium.txt"),
    ("lorem_ipsum_short_txt.sys", "lorem_ipsum_short.txt"),
    ("lorem_ipsum_short_readonly.txt", "lorem_ipsum_short.txt"),
    ("lorem_ipsum_short_hidden.txt", "lorem_ipsum_short.txt"),
    ("lorem_ipsum_short2.txt", "lorem_ipsum_short.txt"),
];

fn reference(path: &str) -> Option<(u64, u32, &'static str)> {
    let base = path.rsplit('/').next()?;
    let base = ALIASES.iter().find(|a| a.0 == base).map_or(base, |a| a.1);
    let &(_, size, crc, sha) = REFERENCE.iter().find(|r| r.0 == base)?;
    let bytes: Vec<u8> = (0..4).map(|i| u8::from_str_radix(&crc[2 * i..2 * i + 2], 16).unwrap()).collect();
    Some((size, u32::from_le_bytes(bytes.try_into().unwrap()), sha))
}

fn error_kind(r: Result<impl std::fmt::Debug>) -> Option<ArchiveError> {
    archive_error(&r.expect_err("expected an error")).cloned()
}

#[test]
fn every_fixture_lists_and_decodes_to_the_reference_contents() {
    let mut on_disk: Vec<String> = fs::read_dir(fixture("")).unwrap().flatten().map(|e| e.file_name().into_string().unwrap()).collect();
    on_disk.sort();
    let table = fixtures();
    let mut named: Vec<String> = table.iter().map(|(n, _)| n.to_string()).collect();
    named.sort();
    assert_eq!(named, on_disk, "every fixture is covered");

    let mut checked_against_reference = 0;
    for (name, expected) in &table {
        let listed = list(&fixture(name), None).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        let got: Vec<(String, u64)> =
            listed.iter().map(|e| (if e.is_dir { format!("{}/", e.path) } else { e.path.clone() }, e.size)).collect();
        let want: Vec<(String, u64)> = expected.iter().map(|&(p, s)| (p.to_string(), s)).collect();
        assert_eq!(got, want, "{name}: listing");
        assert!(listed.iter().all(|e| e.modified.is_some()), "{name}: times");

        let seen = read_fixture(name, Some(PASSWORD)).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        assert_eq!(seen.len(), expected.len());
        for s in &seen {
            let path = &s.item.path;
            if s.item.is_dir {
                assert!(s.contents.is_none());
                continue;
            }
            let c = s.contents.as_ref().unwrap_or_else(|| panic!("{name}/{path}: no data"));
            assert_eq!(c.len, s.item.size, "{name}/{path}: length");
            if let Some((size, crc, sha)) = reference(path) {
                assert_eq!((c.len, c.crc, c.sha.as_str()), (size, crc, sha), "{name}/{path}");
                checked_against_reference += 1;
            }
        }
    }
    assert_eq!(checked_against_reference, 60);

    // The same 7 MB file, once AZO-compressed and once split over two deflate volumes.
    let sha = |name: &str| read_fixture(name, None).unwrap()[0].contents.take().unwrap().sha;
    assert_eq!(sha("azonumbers.egg"), sha("number.vol1.egg"));
}

#[test]
fn methods_and_sizes_are_reported() {
    let methods = |name: &str| -> Vec<String> { list(&fixture(name), None).unwrap().into_iter().map(|e| e.method).collect() };
    assert!(methods("defaults.egg").iter().any(|m| m == "저장"));
    assert!(methods("defaults.egg").iter().any(|m| m == "Deflate"));
    assert!(methods("defaults.alz").iter().all(|m| m == "Deflate"));
    assert!(methods("default_lzma_pri_compress_ratio.egg").iter().all(|m| m == "LZMA"));
    assert!(methods("defaults_bz_optimized.egg").iter().any(|m| m == "BZip2"));
    assert!(methods("azomulti.egg").iter().all(|m| m == "AZO"));
    assert!(methods("solid_defaults.egg").iter().all(|m| m == "Deflate"));

    let zeros = &list(&fixture("zeros.egg"), None).unwrap()[0];
    assert_eq!((zeros.size, zeros.packed, zeros.method.as_str()), (197_540_460, Some(338), "BZip2"));
    let lzma = list(&fixture("lzma_simple.egg"), None).unwrap();
    let lorem = lzma.iter().find(|e| e.path == "lorem_ipsum.txt").unwrap();
    assert_eq!((lorem.size, lorem.packed), (34_446, Some(5_723)));
    assert!(list(&fixture("solid.egg"), None).unwrap().iter().all(|e| e.packed.is_none()));
}

#[test]
fn contents_match_eggdotnet_assertions() {
    let text = |name: &str, path: &str| -> String {
        let seen = read_fixture(name, Some(PASSWORD)).unwrap();
        let s = seen.iter().find(|s| s.item.path == path).unwrap_or_else(|| panic!("{name}: {path}"));
        String::from_utf8(s.contents.as_ref().unwrap().bytes.clone()).unwrap()
    };
    for name in ["lorem_long_zipEnc.egg", "lorem_long_aes128.egg", "lorem_long_aes256.egg"] {
        let t = text(name, "lorem_ipsum_long.txt");
        assert_eq!(t.len(), 15_238);
        assert!(t.starts_with("Lorem ipsum dolor sit amet") && t.ends_with("sed faucibus orci ligula eu nisi."), "{name}");
    }
    let t = text("basic_deflate_folders.egg", "lorems/lorem_ipsum.txt");
    assert!(t.len() == 34_446 && t.starts_with("Lorem ipsum dolor sit amet") && t.ends_with("gravida."));
    assert_eq!(text("lzma_simple.egg", "lorem_ipsum.txt").len(), 34_446);
    assert_eq!(text("small_kor.egg", "작은.txt"), "Hello, world!");
    assert_eq!(text("small_shift.egg", "小さい.txt"), "Hello, world!");
    assert_eq!(text("solid.egg", "s/lorem_ipsum_short.txt"), text("solid.egg", "s/lorem_ipsum_short2.txt"));
}

#[test]
fn passwords() {
    for &(name, cipher) in ENCRYPTED {
        let listed = list(&fixture(name), None).unwrap_or_else(|e| panic!("{name}: listing needs no password: {e:#}"));
        assert!(listed.iter().all(|e| e.encrypted), "{name}");
        assert_eq!(error_kind(read_fixture(name, None)), Some(ArchiveError::PasswordRequired), "{name} ({cipher})");
        assert_eq!(error_kind(read_fixture(name, Some(""))), Some(ArchiveError::PasswordRequired), "{name} ({cipher})");
        for wrong in ["password12345", "badpassword", "암호"] {
            assert_eq!(error_kind(read_fixture(name, Some(wrong))), Some(ArchiveError::WrongPassword), "{name} ({cipher}) {wrong}");
        }
        let seen = read_fixture(name, Some(PASSWORD)).unwrap_or_else(|e| panic!("{name} ({cipher}): {e:#}"));
        assert!(seen.iter().all(|s| s.contents.is_some()), "{name}");
    }
    let plain = read_fixture("defaults.egg", None).unwrap();
    assert!(plain.iter().all(|s| !s.item.encrypted && s.contents.is_some()));
}

#[test]
fn extract_through_the_planner() {
    let dir = tempfile::tempdir().unwrap();
    let out = |sub: &str| dir.path().join(sub);

    let r = extract(&fixture("directories.egg"), &out("d"), None, None).unwrap();
    assert_eq!(fs::read(out("d/dir1/dir2/dir3/test.txt")).unwrap().len(), 18);
    assert_eq!(r.files, 1);

    extract(&fixture("small_kor.egg"), &out("k"), None, None).unwrap();
    assert_eq!(fs::read_to_string(out("k/작은.txt")).unwrap(), "Hello, world!");
    extract(&fixture("small_shift.egg"), &out("j"), None, None).unwrap();
    assert_eq!(fs::read_to_string(out("j/小さい.txt")).unwrap(), "Hello, world!");

    let r = extract(&fixture("extattr.egg"), &out("x"), None, None).unwrap();
    assert!(out("x/Some folder").is_dir());
    assert_eq!((r.files, r.dirs), (4, 1));

    for name in ["defaults.alz", "solid_defaults.egg", "lorem_long_store_lea256.egg"] {
        let dest = out(name);
        extract(&fixture(name), &dest, None, Some(PASSWORD)).unwrap();
        for e in list(&fixture(name), None).unwrap() {
            let bytes = fs::read(dest.join(&e.path)).unwrap();
            let (size, _, sha) = reference(&e.path).unwrap();
            assert_eq!((bytes.len() as u64, sha_hex(&bytes)), (size, sha.to_string()), "{name}/{}", e.path);
        }
    }

    let r = extract(&fixture("number.vol1.egg"), &out("n"), None, None).unwrap();
    assert_eq!((r.files, r.bytes), (1, 6_999_999));
    assert_eq!(sha_hex(&fs::read(out("n/number.txt")).unwrap()), "AF5DAB14D15E1377C0AC3CA1B217296A87BE5CC3CD3A063CC3C5A937E0ACB364");

    let failed = extract(&fixture("lorem_long_aes128.egg"), &out("p"), None, None);
    assert_eq!(error_kind(failed), Some(ArchiveError::PasswordRequired));
}

#[test]
fn selected_entries_from_a_solid_archive() {
    let dir = tempfile::tempdir().unwrap();
    for pick in ["s2/lorem_ipsum_short.txt", "s2/lorem_ipsum_long.txt", "s2/lorem_ipsum.zip"] {
        let dest = dir.path().join(pick.replace('/', "_"));
        let r = extract(&fixture("solid_defaults.egg"), &dest, Some(vec![pick.to_string()]), None).unwrap();
        assert_eq!(r.files, 1);
        let bytes = fs::read(dest.join(pick)).unwrap();
        assert_eq!(sha_hex(&bytes), reference(pick).unwrap().2, "{pick}");
    }
}

#[test]
fn split_egg_volumes_are_found_and_required() {
    for name in ["number.vol1.egg", "number.vol2.egg"] {
        let seen = read_fixture(name, None).unwrap();
        assert_eq!(seen[0].contents.as_ref().unwrap().len, 6_999_999);
    }
    let dir = tempfile::tempdir().unwrap();
    let alone = dir.path().join("number.vol1.egg");
    fs::copy(fixture("number.vol1.egg"), &alone).unwrap();
    let err = read_archive(&alone, None).expect_err("second volume is missing");
    assert!(format!("{err:#}").contains("volume") || format!("{err:#}").contains("분할"), "{err:#}");
}

/// A one-entry-per-file ALZ archive written the way unalz reads it.
struct AlzEntry<'a> {
    name: &'a [u8],
    attr: u8,
    method: u8,
    data: &'a [u8],
    packed: Vec<u8>,
    encrypt_with: Option<&'a [u8]>,
}

fn crc32(data: &[u8]) -> u32 {
    let mut h = crc32fast::Hasher::new();
    h.update(data);
    h.finalize()
}

/// PKWARE traditional encryption, for building test archives.
fn zip_encrypt(password: &[u8], check: u8, data: &[u8]) -> Vec<u8> {
    let mut keys = [0x1234_5678u32, 0x2345_6789, 0x3456_7890];
    let table: Vec<u32> = (0..256u32).map(|i| (0..8).fold(i, |c, _| if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 })).collect();
    let update = |keys: &mut [u32; 3], b: u8| {
        keys[0] = table[((keys[0] ^ b as u32) & 0xFF) as usize] ^ (keys[0] >> 8);
        keys[1] = keys[1].wrapping_add(keys[0] & 0xFF).wrapping_mul(134_775_813).wrapping_add(1);
        keys[2] = table[((keys[2] ^ (keys[1] >> 24)) & 0xFF) as usize] ^ (keys[2] >> 8);
    };
    password.iter().for_each(|&b| update(&mut keys, b));
    let mut header = *b"rustzip-alz?";
    header[11] = check;
    header.iter().chain(data).map(|&p| {
        let t = (keys[2] | 2) & 0xFFFF;
        let c = p ^ ((t.wrapping_mul(t ^ 1) >> 8) as u8);
        update(&mut keys, p);
        c
    }).collect()
}

fn alz_archive(entries: &[AlzEntry]) -> Vec<u8> {
    let mut out = b"ALZ\x01\x0a\x00\x00\x00".to_vec();
    for e in entries {
        let crc = crc32(e.data);
        let mut payload = e.packed.clone();
        let mut flags = 0x40;
        if let Some(pw) = e.encrypt_with {
            payload = zip_encrypt(pw, (crc >> 24) as u8, &payload);
            flags |= 1;
        }
        out.extend_from_slice(b"BLZ\x01");
        out.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
        out.push(e.attr);
        out.extend_from_slice(&0x579D_6971u32.to_le_bytes());
        if e.attr & 0x10 != 0 {
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(e.name);
            continue;
        }
        out.extend_from_slice(&[flags, 0, e.method, 0]);
        out.extend_from_slice(&crc.to_le_bytes());
        let packed_len = (payload.len() - if e.encrypt_with.is_some() { 12 } else { 0 }) as u32;
        out.extend_from_slice(&packed_len.to_le_bytes());
        out.extend_from_slice(&(e.data.len() as u32).to_le_bytes());
        out.extend_from_slice(e.name);
        out.extend_from_slice(&payload);
    }
    out.extend_from_slice(b"CLZ\x01");
    out.extend_from_slice(&[0; 8]);
    out.extend_from_slice(b"CLZ\x02");
    out
}

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

fn alz_bzip2(data: &[u8]) -> Vec<u8> {
    let mut standard = Vec::new();
    bzip2::read::BzEncoder::new(data, bzip2::Compression::best()).read_to_end(&mut standard).unwrap();
    to_alz_framing(&standard)
}

#[test]
fn alz_bzip2_encryption_folders_and_cp949_names() {
    let text: Vec<u8> = (0..200_000u32).flat_map(|i| format!("{} 줄\n", i % 1777).into_bytes()).collect();
    let small = b"hello alz".to_vec();
    let entries = [
        AlzEntry { name: "폴더".as_bytes(), attr: 0x10, method: 0, data: b"", packed: vec![], encrypt_with: None },
        AlzEntry { name: b"bz.txt", attr: 0x20, method: 1, data: &text, packed: alz_bzip2(&text), encrypt_with: None },
        AlzEntry { name: &encoding_rs::EUC_KR.encode("폴더\\한글.txt").0, attr: 0x20, method: 2, data: &text, packed: deflate(&text), encrypt_with: None },
        AlzEntry { name: b"stored.txt", attr: 0x20, method: 0, data: &small, packed: small.clone(), encrypt_with: Some(b"secret") },
        AlzEntry { name: b"secret.txt", attr: 0x20, method: 1, data: &text, packed: alz_bzip2(&text), encrypt_with: Some(b"secret") },
    ];
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("made.alz");
    fs::write(&path, alz_archive(&entries)).unwrap();

    let listed = list(&path, None).unwrap();
    let names: Vec<_> = listed.iter().map(|e| (e.path.as_str(), e.is_dir, e.encrypted, e.method.as_str())).collect();
    assert_eq!(
        names,
        [
            ("폴더", true, false, "저장"),
            ("bz.txt", false, false, "BZip2"),
            ("폴더/한글.txt", false, false, "Deflate"),
            ("stored.txt", false, true, "저장"),
            ("secret.txt", false, true, "BZip2"),
        ]
    );
    assert_eq!(listed[0].modified.unwrap().to_string(), "2023-12-29 13:11:34");

    assert_eq!(error_kind(read_archive(&path, None)), Some(ArchiveError::PasswordRequired));
    assert_eq!(error_kind(read_archive(&path, Some("wrong"))), Some(ArchiveError::WrongPassword));
    let seen = read_archive(&path, Some("secret")).unwrap();
    let sha = |i: usize| seen[i].contents.as_ref().unwrap().sha.clone();
    assert_eq!(sha(1), sha_hex(&text));
    assert_eq!(sha(2), sha_hex(&text));
    assert_eq!(seen[3].contents.as_ref().unwrap().bytes, small);
    assert_eq!(sha(4), sha_hex(&text));

    let r = extract(&path, &dir.path().join("out"), None, Some("secret")).unwrap();
    assert_eq!((r.files, r.dirs), (4, 1));
    assert_eq!(fs::read(dir.path().join("out/폴더/한글.txt")).unwrap(), text);
}

#[test]
fn alz_split_volumes() {
    let whole = fs::read(fixture("defaults.alz")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cuts = [whole.len() / 3, 2 * whole.len() / 3];
    let pieces = [&whole[..cuts[0]], &whole[cuts[0]..cuts[1]], &whole[cuts[1]..]];
    for (i, (piece, name)) in pieces.iter().zip(["split.alz", "split.a00", "split.a01"]).enumerate() {
        let mut v = Vec::new();
        if i > 0 {
            v.extend_from_slice(&[0xAA; 8]);
        }
        v.extend_from_slice(piece);
        if i < 2 {
            v.extend_from_slice(&[0xBB; 16]);
        }
        fs::write(dir.path().join(name), v).unwrap();
    }
    let seen = read_archive(&dir.path().join("split.alz"), None).unwrap();
    assert_eq!(seen.len(), 6);
    for s in &seen {
        let (size, crc, sha) = reference(&s.item.path).unwrap();
        let c = s.contents.as_ref().unwrap();
        assert_eq!((c.len, c.crc, c.sha.as_str()), (size, crc, sha), "{}", s.item.path);
    }
    fs::remove_file(dir.path().join("split.a01")).unwrap();
    assert!(read_archive(&dir.path().join("split.alz"), None).is_err());
}

/// Damaged archives must fail with an error, never panic or hang.
#[test]
fn damaged_archives_fail_cleanly() {
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let dir = tempfile::tempdir().unwrap();
    let names = [
        "azoshort_txt.egg", "azomulti.egg", "lorem_long_aes128.egg", "lorem_long_store_lea256.egg", "lorem_long_zipEnc.egg",
        "lzma_simple.egg", "solid.egg", "small_kor.egg", "extattr.egg", "posix_small.egg", "defaults.alz",
    ];
    for name in names {
        let good = fs::read(fixture(name)).unwrap();
        let ext = if name.ends_with(".alz") { "alz" } else { "egg" };
        let path = dir.path().join(format!("bad.{ext}"));
        let tries = if good.len() > 100_000 { 40 } else { 300 };
        for t in 0..tries {
            let mut bad = good.clone();
            if t % 4 == 0 {
                bad.truncate((next() % good.len() as u64) as usize);
            } else {
                // Damage mostly the headers and the first data bytes.
                let span = good.len().min(4096) as u64;
                for _ in 0..1 + t % 3 {
                    let at = (next() % span) as usize;
                    bad[at] ^= 1 << (next() % 8);
                }
            }
            fs::write(&path, &bad).unwrap();
            let _ = list(&path, None);
            let _ = read_archive(&path, Some(PASSWORD));
        }
    }
}

#[test]
fn damaged_data_is_caught_by_the_crc() {
    let dir = tempfile::tempdir().unwrap();
    let mut egg = fs::read(fixture("defaults_nocompress.egg")).unwrap();
    let at = egg.windows(11).position(|w| w == b"Lorem ipsum").unwrap() + 100;
    egg[at] ^= 0x20;
    let path = dir.path().join("bad.egg");
    fs::write(&path, &egg).unwrap();
    let err = read_archive(&path, None).expect_err("stored data was changed");
    assert!(format!("{err:#}").contains("CRC"), "{err:#}");

    let data = b"stored alz data".to_vec();
    let mut alz = alz_archive(&[AlzEntry { name: b"a.txt", attr: 0x20, method: 0, data: &data, packed: data.clone(), encrypt_with: None }]);
    let at = alz.windows(6).position(|w| w == b"stored").unwrap();
    alz[at] ^= 0x20;
    let path = dir.path().join("bad.alz");
    fs::write(&path, &alz).unwrap();
    let err = read_archive(&path, None).expect_err("stored data was changed");
    assert!(format!("{err:#}").contains("CRC"), "{err:#}");
}
