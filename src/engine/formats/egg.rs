//! EGG, ALZip's newer format (read only), after ESTsoft's EGG specification 1.0.
//!
//! Every header is a signature followed by fields, and extra fields
//! (signature, flag, size, data) repeat until the end signature 0x08E28222.
//! The archive is: EGG header (+ split/solid fields), then per file a file
//! header with its extra fields and one or more blocks (method, sizes, CRC,
//! data). Solid archives list every file header first and then the blocks of
//! one shared stream. Split archives are separate files, each with its own EGG
//! header, linked by header IDs; the archive is what follows those headers.

use std::fs::{self, File};
use std::io::{self, BufReader, Read, Seek};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{bail, Result};
use chrono::{DateTime, Local, NaiveDateTime};
use flate2::read::DeflateDecoder;

use super::super::codec::azo::AzoReader;
use super::super::codec::ctr::{self, CtrReader};
use super::super::codec::verify::Verify;
use super::super::codec::zipcrypto::{self, ZipCryptoReader};
use super::super::walk::{cancel_aware, Item, Source, Visitor};
use super::super::{decode_name, ArchiveError};
use super::volumes::{ReadLe, Volumes};

const EGG: u32 = 0x4147_4745;
const END: u32 = 0x08E2_8222;
const SPLIT: u32 = 0x24F5_A262;
const SOLID: u32 = 0x24E5_A060;
const GLOBAL_ENCRYPT: u32 = 0x08D1_44A8;
const FILE: u32 = 0x0A85_90E3;
const BLOCK: u32 = 0x02B5_0C13;
const ENCRYPT: u32 = 0x08D1_470F;
const WINDOWS_INFO: u32 = 0x2C86_950B;
const POSIX_INFO: u32 = 0x1EE9_22E5;
const FILENAME: u32 = 0x0A85_91AC;

const NAME_ENCRYPTED: u8 = 0x04;
const NAME_CODEPAGE: u8 = 0x08;
const NAME_PARENT: u8 = 0x10;
const WINDOWS_DIRECTORY: u8 = 0x80;
const POSIX_TYPE_MASK: u32 = 0o170000;
const POSIX_DIRECTORY: u32 = 0o040000;

fn damaged(what: &str) -> anyhow::Error {
    anyhow::anyhow!(trf!("EGG 압축 파일이 손상되었습니다 ({what}).", "The EGG archive is damaged ({what})."))
}

/// Size of an extra field's data: 2 bytes, or 4 when bit 0 of its flag is set.
fn field_size(r: &mut impl Read, flag: u8) -> io::Result<u64> {
    if flag & 1 != 0 {
        r.u32().map(u64::from)
    } else {
        r.u16().map(u64::from)
    }
}

struct VolumeHeader {
    id: u32,
    /// (previous, next) volume IDs; 0 means none.
    split: Option<(u32, u32)>,
    solid: bool,
    /// Where the archive data of this volume starts.
    end: u64,
}

fn read_volume_header(path: &Path) -> Result<VolumeHeader> {
    let mut r = BufReader::new(File::open(path)?);
    if r.u32()? != EGG {
        bail!(tr!("EGG 압축 파일이 아닙니다.", "This is not an EGG archive."));
    }
    let version = r.u16()?;
    if version >> 8 != 1 {
        return Err(ArchiveError::Unsupported(format!("EGG {}.{}", version >> 8, version & 0xFF)).into());
    }
    let id = r.u32()?;
    let _reserved = r.u32()?;
    let (mut split, mut solid) = (None, false);
    loop {
        let sig = r.u32()?;
        if sig == END {
            break;
        }
        let flag = r.u8()?;
        let size = field_size(&mut r, flag)?;
        match sig {
            SPLIT if size >= 8 => {
                split = Some((r.u32()?, r.u32()?));
                r.seek_relative(size as i64 - 8)?;
            }
            SOLID => {
                solid = true;
                r.seek_relative(size as i64)?;
            }
            GLOBAL_ENCRYPT => {
                return Err(ArchiveError::Unsupported(tr!("이름까지 암호화된 EGG", "EGG with encrypted headers").into()).into())
            }
            _ => r.seek_relative(size as i64)?,
        }
    }
    Ok(VolumeHeader { id, split, solid, end: r.stream_position()? })
}

/// The other volumes a split archive could be in: `x.vol2.egg` next to `x.vol1.egg`,
/// or every EGG file in the folder when the name does not follow that pattern.
fn sibling_volumes(path: &Path) -> Vec<PathBuf> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let prefix = name.rfind(".vol").map(|i| name[..i + 4].to_string()).unwrap_or_default();
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            p != path && n.ends_with(".egg") && n.starts_with(&prefix)
        })
        .collect()
}

/// The archive's volumes in order, and whether it is solid.
fn open_volumes<'p>(src: &Source<'p>) -> Result<(Volumes<'p>, bool)> {
    let opened = read_volume_header(src.path)?;
    let mut chain = vec![(src.path.to_path_buf(), opened)];
    if chain[0].1.split.is_some() {
        let mut others: Vec<(PathBuf, VolumeHeader)> =
            sibling_volumes(src.path).into_iter().filter_map(|p| Some((p.clone(), read_volume_header(&p).ok()?))).collect();
        let mut take = |id: u32| {
            let i = others.iter().position(|(_, h)| h.id == id && h.split.is_some());
            i.map(|i| others.swap_remove(i))
        };
        while let Some((prev, _)) = chain[0].1.split.filter(|&(prev, _)| prev != 0) {
            let Some(v) = take(prev) else { bail!(tr!("앞쪽 분할 파일을 찾을 수 없습니다.", "An earlier volume of this split archive is missing.")) };
            chain.insert(0, v);
        }
        while let Some((_, next)) = chain[chain.len() - 1].1.split.filter(|&(_, next)| next != 0) {
            let Some(v) = take(next) else { bail!(tr!("다음 분할 파일을 찾을 수 없습니다.", "The next volume of this split archive is missing.")) };
            chain.push(v);
        }
    }
    let solid = chain[0].1.solid;
    let mut vols = Volumes::new(src.input);
    for (path, header) in chain {
        let file = File::open(&path)?;
        let size = file.metadata()?.len();
        vols.push(file, header.end, size.saturating_sub(header.end));
    }
    Ok((vols, solid))
}

#[derive(Clone)]
enum Crypt {
    Zip { header: [u8; 12], check: u8 },
    Ctr { kind: ctr::Kind, salt: Vec<u8>, check: [u8; 2], mac: [u8; 10] },
    Unknown(u8),
}

struct Block {
    method: u8,
    size: u32,
    packed: u32,
    crc: u32,
    data: u64,
}

struct FileEntry {
    item: Item,
    crypt: Option<Crypt>,
    blocks: Vec<Block>,
}

fn method_name(m: u8) -> String {
    match m {
        0 => "저장".into(),
        1 => "Deflate".into(),
        2 => "BZip2".into(),
        3 => "AZO".into(),
        4 => "LZMA".into(),
        n => format!("Method {n}"),
    }
}

fn decode_with_codepage(raw: &[u8], codepage: u16, src: &Source<'_>) -> String {
    let enc = match codepage {
        932 => encoding_rs::SHIFT_JIS,
        936 => encoding_rs::GBK,
        949 => encoding_rs::EUC_KR,
        950 => encoding_rs::BIG5,
        65001 => encoding_rs::UTF_8,
        _ => return decode_name(raw, false, src.encoding),
    };
    enc.decode(raw).0.into_owned()
}

fn filetime(t: u64) -> Option<NaiveDateTime> {
    let secs = (t / 10_000_000) as i64 - 11_644_473_600;
    unix_time(secs, (t % 10_000_000) as u32 * 100)
}

fn unix_time(secs: i64, nanos: u32) -> Option<NaiveDateTime> {
    Some(DateTime::from_timestamp(secs, nanos)?.with_timezone(&Local).naive_local())
}

fn read_crypt(data: &[u8]) -> Option<Crypt> {
    let (&method, rest) = data.split_first()?;
    let ctr_kind = match method {
        0 => {
            let header = rest.get(..12)?.try_into().ok()?;
            let check = *rest.get(15)?;
            return Some(Crypt::Zip { header, check });
        }
        1 => ctr::Kind::Aes128,
        2 => ctr::Kind::Aes256,
        5 => ctr::Kind::Lea128,
        6 => ctr::Kind::Lea256,
        m => return Some(Crypt::Unknown(m)),
    };
    let n = ctr_kind.salt_len();
    Some(Crypt::Ctr {
        kind: ctr_kind,
        salt: rest.get(..n)?.to_vec(),
        check: rest.get(n..n + 2)?.try_into().ok()?,
        mac: rest.get(n + 2..n + 12)?.try_into().ok()?,
    })
}

fn read_file_header(r: &mut BufReader<impl Read + Seek>, src: &Source<'_>) -> Result<FileEntry> {
    let _id = r.u32()?;
    let size = r.u64()?;
    let mut name = String::new();
    let (mut is_dir, mut modified, mut crypt) = (false, None, None);
    loop {
        let sig = r.u32()?;
        if sig == END {
            break;
        }
        let flag = r.u8()?;
        let len = field_size(r, flag)?;
        match sig {
            FILENAME => {
                if flag & NAME_ENCRYPTED != 0 {
                    return Err(ArchiveError::Unsupported(tr!("암호화된 파일 이름", "encrypted file names").into()).into());
                }
                let codepage = if flag & NAME_CODEPAGE != 0 { Some(r.u16()?) } else { None };
                if flag & NAME_PARENT != 0 {
                    r.u32()?;
                }
                let raw = r.vec(len as usize)?;
                name = match codepage {
                    Some(cp) => decode_with_codepage(&raw, cp, src),
                    None => String::from_utf8_lossy(&raw).into_owned(),
                };
            }
            WINDOWS_INFO if len >= 9 => {
                let data = r.vec(len as usize)?;
                modified = filetime(u64::from_le_bytes(data[..8].try_into()?));
                is_dir |= data[8] & WINDOWS_DIRECTORY != 0;
            }
            POSIX_INFO if len >= 20 => {
                let data = r.vec(len as usize)?;
                let mode = u32::from_le_bytes(data[..4].try_into()?);
                modified = unix_time(i64::from_le_bytes(data[12..20].try_into()?), 0);
                is_dir |= mode & POSIX_TYPE_MASK == POSIX_DIRECTORY;
            }
            ENCRYPT => {
                let data = r.vec(len as usize)?;
                crypt = Some(read_crypt(&data).ok_or_else(|| damaged("encryption header"))?);
            }
            _ => r.seek_relative(len as i64)?,
        }
    }
    is_dir |= name.ends_with('/') || name.ends_with('\\');
    let mut item = Item::new(&name, is_dir);
    item.size = size;
    item.modified = modified;
    item.encrypted = crypt.is_some();
    Ok(FileEntry { item, crypt, blocks: Vec::new() })
}

fn read_block(r: &mut BufReader<impl Read + Seek>) -> Result<Block> {
    let method = r.u8()?;
    let _hint = r.u8()?;
    let size = r.u32()?;
    let packed = r.u32()?;
    let crc = r.u32()?;
    loop {
        let sig = r.u32()?;
        if sig == END {
            break;
        }
        let flag = r.u8()?;
        let len = field_size(r, flag)?;
        r.seek_relative(len as i64)?;
    }
    let data = r.stream_position()?;
    r.seek_relative(i64::from(packed))?;
    Ok(Block { method, size, packed, crc, data })
}

struct Archive {
    files: Vec<FileEntry>,
    /// The shared stream of a solid archive.
    solid: Option<Vec<Block>>,
}

fn read_archive(vols: &Volumes<'_>, solid: bool, src: &Source<'_>) -> Result<Archive> {
    let mut r = vols.reader(0);
    let mut archive = Archive { files: Vec::new(), solid: solid.then(Vec::new) };
    while let Some(sig) = r.signature()? {
        match sig {
            END => break,
            FILE => archive.files.push(read_file_header(&mut r, src)?),
            BLOCK => {
                let block = read_block(&mut r)?;
                match (&mut archive.solid, archive.files.last_mut()) {
                    (Some(shared), _) => shared.push(block),
                    (None, Some(file)) => file.blocks.push(block),
                    (None, None) => return Err(damaged("block before any file")),
                }
            }
            _ => {
                let flag = r.u8()?;
                let len = field_size(&mut r, flag)?;
                r.seek_relative(len as i64)?;
            }
        }
    }
    Ok(archive)
}

/// Decryption ready to apply to each block.
#[derive(Clone)]
enum Keys {
    Zip(zipcrypto::Keys),
    Ctr(Rc<ctr::Keys>, [u8; 10]),
}

impl Keys {
    fn for_entry(crypt: &Option<Crypt>, src: &Source<'_>) -> Result<Option<Keys>> {
        let Some(crypt) = crypt else { return Ok(None) };
        let keys = match crypt {
            Crypt::Unknown(m) => {
                return Err(ArchiveError::Unsupported(trf!("EGG 암호화 방식 {m}", "EGG encryption method {m}")).into())
            }
            Crypt::Zip { header, check } => {
                let mut keys = zipcrypto::Keys::new(&zipcrypto::password_bytes(src.require_password()?));
                (keys.decrypt_header(header) == *check).then_some(Keys::Zip(keys))
            }
            Crypt::Ctr { kind, salt, check, mac } => ctr::Keys::derive(*kind, src.require_password()?.as_bytes(), salt, *check)
                .map(|k| Keys::Ctr(Rc::new(k), *mac)),
        };
        keys.map(Some).ok_or_else(|| ArchiveError::WrongPassword.into())
    }
}

const KNOWN_METHODS: std::ops::RangeInclusive<u8> = 0..=4;

fn check_methods(blocks: &[Block]) -> Result<()> {
    match blocks.iter().find(|b| !KNOWN_METHODS.contains(&b.method)) {
        Some(b) => Err(ArchiveError::Unsupported(trf!("EGG 압축 방식 {}", "EGG compression method {}", b.method)).into()),
        None => Ok(()),
    }
}

fn open_block<'a>(vols: &'a Volumes<'a>, b: &Block, keys: &Option<Keys>) -> Result<Verify<Box<dyn Read + 'a>>> {
    let raw = vols.slice(b.data, u64::from(b.packed));
    let mut r: Box<dyn Read + 'a> = match keys.clone() {
        None => Box::new(raw),
        Some(Keys::Zip(k)) => Box::new(ZipCryptoReader::new(raw, k)),
        Some(Keys::Ctr(k, mac)) => Box::new(CtrReader::new(raw, k, mac)),
    };
    let size = u64::from(b.size);
    let decoded: Box<dyn Read + 'a> = match b.method {
        0 => r,
        1 => Box::new(DeflateDecoder::new(r)),
        2 => Box::new(bzip2::read::BzDecoder::new(r)),
        3 => Box::new(AzoReader::new(r, size)),
        4 => {
            let head: [u8; 4] = r.array()?;
            if u16::from_le_bytes([head[2], head[3]]) != 5 {
                return Err(damaged("LZMA header"));
            }
            let props: [u8; 5] = r.array()?;
            let dict = u32::from_le_bytes([props[1], props[2], props[3], props[4]]).min(b.size.max(4096));
            Box::new(lzma_rust2::LzmaReader::new_with_props(r, size, props[0], dict, None)?)
        }
        m => return Err(ArchiveError::Unsupported(trf!("EGG 압축 방식 {m}", "EGG compression method {m}")).into()),
    };
    Ok(Verify::new(decoded, size, Some(b.crc)))
}

/// The decoded blocks of one file, or of a solid archive's shared stream, in order.
struct Blocks<'a> {
    vols: &'a Volumes<'a>,
    blocks: &'a [Block],
    keys: Option<Keys>,
    current: Option<Verify<Box<dyn Read + 'a>>>,
    next: usize,
    failed: bool,
}

impl<'a> Blocks<'a> {
    fn new(vols: &'a Volumes<'a>, blocks: &'a [Block], keys: Option<Keys>) -> Self {
        Blocks { vols, blocks, keys, current: None, next: 0, failed: false }
    }

    /// ZipCrypto's 1-byte password check lets 1 in 256 wrong passwords through;
    /// the bad data they produce is reported as a wrong password too.
    fn explain(&self, e: anyhow::Error) -> anyhow::Error {
        if self.failed && matches!(self.keys, Some(Keys::Zip(_))) {
            ArchiveError::WrongPassword.into()
        } else {
            e
        }
    }
}

impl Read for Blocks<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.current.is_none() {
                let Some(b) = self.blocks.get(self.next) else { return Ok(0) };
                self.next += 1;
                let opened = open_block(self.vols, b, &self.keys).map_err(|e| {
                    self.failed = true;
                    io::Error::other(e)
                })?;
                self.current = Some(opened);
            }
            let Some(current) = &mut self.current else { return Ok(0) };
            match current.read(buf) {
                Ok(0) if !buf.is_empty() => self.current = None,
                Ok(n) => return Ok(n),
                Err(e) => {
                    self.failed |= current.failed;
                    return Err(e);
                }
            }
        }
    }
}

fn walk_files(vols: &Volumes<'_>, files: &[FileEntry], src: &Source<'_>, v: &mut dyn Visitor) -> Result<()> {
    for f in files {
        let mut item = f.item.clone();
        item.packed = Some(f.blocks.iter().map(|b| u64::from(b.packed)).sum());
        item.method = f.blocks.first().map(|b| method_name(b.method)).unwrap_or_default();
        if !v.entry(&item)? || item.is_dir {
            continue;
        }
        check_methods(&f.blocks)?;
        let mut blocks = Blocks::new(vols, &f.blocks, Keys::for_entry(&f.crypt, src)?);
        let res = v.data(&item, &mut Verify::new(&mut blocks, item.size, None));
        res.map_err(|e| blocks.explain(e))?;
    }
    Ok(())
}

fn walk_solid(vols: &Volumes<'_>, files: &[FileEntry], blocks: &[Block], src: &Source<'_>, v: &mut dyn Visitor) -> Result<()> {
    let method = method_name(blocks.first().map_or(0, |b| b.method));
    let crypt = files.iter().find_map(|f| f.crypt.clone());
    let mut stream: Option<Blocks<'_>> = None;
    let mut skip = 0u64;
    for f in files {
        let mut item = f.item.clone();
        item.method = method.clone();
        if !v.entry(&item)? || item.is_dir {
            skip += item.size;
            continue;
        }
        let shared = match &mut stream {
            Some(s) => s,
            None => {
                check_methods(blocks)?;
                stream.insert(Blocks::new(vols, blocks, Keys::for_entry(&crypt, src)?))
            }
        };
        let skipped = io::copy(&mut shared.take(skip), &mut io::sink()).map_err(|e| shared.explain(e.into()))?;
        if skipped != skip {
            return Err(damaged("solid stream ends early"));
        }
        let mut part = shared.take(item.size);
        let res = v.data(&item, &mut Verify::new(&mut part, item.size, None));
        skip = part.limit();
        res.map_err(|e| shared.explain(e))?;
    }
    Ok(())
}

fn walk_archive(src: &Source<'_>, v: &mut dyn Visitor) -> Result<()> {
    let (vols, solid) = open_volumes(src)?;
    let archive = read_archive(&vols, solid, src)?;
    match &archive.solid {
        Some(blocks) => walk_solid(&vols, &archive.files, blocks, src, v),
        None => walk_files(&vols, &archive.files, src, v),
    }
}

pub fn walk(src: &Source<'_>, v: &mut dyn Visitor) -> Result<()> {
    walk_archive(src, v).map_err(|e| cancel_aware(e, src.input))
}
