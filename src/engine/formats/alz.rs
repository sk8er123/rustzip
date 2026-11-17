//! ALZ, ALZip's original format (read only).
//!
//! Layout: "ALZ\x01" + version/id, then per entry "BLZ\x01" + local header + name
//! (+ 12-byte ZipCrypto header when encrypted) + data, then the "CLZ\x01" and
//! "CLZ\x02" trailer, which lists nothing the local headers do not. Split archives
//! are x.alz, x.a00, x.a01 …: every volume but the last ends with a 16-byte tail
//! and every volume but the first starts with an 8-byte head; the archive is the
//! concatenation of what lies between.

use std::fs::File;
use std::io::{self, Read, Seek};
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use chrono::{NaiveDate, NaiveDateTime};
use flate2::read::DeflateDecoder;

use super::super::codec::alzbzip2::{Bzip2Reader, Framing};
use super::super::codec::verify::Verify;
use super::super::codec::zipcrypto::{self, Keys, ZipCryptoReader};
use super::super::walk::{cancel_aware, Item, Source, Visitor};
use super::super::{decode_name, ArchiveError};
use super::volumes::{ReadLe, Volumes};

const ARCHIVE: u32 = u32::from_le_bytes(*b"ALZ\x01");
const LOCAL: u32 = u32::from_le_bytes(*b"BLZ\x01");
const CENTRAL: u32 = u32::from_le_bytes(*b"CLZ\x01");
const END: u32 = u32::from_le_bytes(*b"CLZ\x02");

const ATTR_DIRECTORY: u8 = 0x10;
const FLAG_ENCRYPTED: u8 = 0x01;
const FLAG_DATA_DESCRIPTOR: u8 = 0x08;

const VOLUME_HEAD: u64 = 8;
const VOLUME_TAIL: u64 = 16;

fn damaged(what: &str) -> anyhow::Error {
    anyhow::anyhow!(trf!("ALZ 압축 파일이 손상되었습니다 ({what}).", "The ALZ archive is damaged ({what})."))
}

/// x.alz, then x.a00 … x.a99, x.b00 … for as long as they exist.
fn volume_paths(first: &Path) -> Vec<PathBuf> {
    let mut paths = vec![first.to_path_buf()];
    let Some(ext) = first.extension().and_then(|e| e.to_str()) else { return paths };
    if !ext.eq_ignore_ascii_case("alz") {
        return paths;
    }
    let upper = ext.starts_with('A');
    for i in 0..26 * 100 {
        let letter = (b'a' + (i / 100) as u8) as char;
        let name = format!("{}{:02}", if upper { letter.to_ascii_uppercase() } else { letter }, i % 100);
        let p = first.with_extension(name);
        if !p.is_file() {
            break;
        }
        paths.push(p);
    }
    paths
}

fn open_volumes<'p>(src: &Source<'p>) -> Result<Volumes<'p>> {
    let paths = volume_paths(src.path);
    let last = paths.len() - 1;
    let mut vols = Volumes::new(src.input);
    for (i, p) in paths.iter().enumerate() {
        let file = File::open(p)?;
        let size = file.metadata()?.len();
        let head = if i == 0 { 0 } else { VOLUME_HEAD };
        let tail = if i == last { 0 } else { VOLUME_TAIL };
        let Some(len) = size.checked_sub(head + tail) else { return Err(damaged(&p.display().to_string())) };
        vols.push(file, head, len);
    }
    Ok(vols)
}

/// DOS date and time (local time), date in the high half.
fn dos_time(t: u32) -> Option<NaiveDateTime> {
    let (date, time) = (t >> 16, t & 0xFFFF);
    NaiveDate::from_ymd_opt(1980 + (date >> 9) as i32, (date >> 5) & 0xF, date & 0x1F)?
        .and_hms_opt(time >> 11, (time >> 5) & 0x3F, (time & 0x1F) * 2)
}

struct Local {
    item: Item,
    method: u8,
    crc: u32,
    flags: u8,
    time: u32,
    encryption_header: Option<[u8; 12]>,
    data: u64,
}

fn method_name(m: u8) -> String {
    match m {
        0 => "저장".into(),
        1 => "BZip2".into(),
        2 => "Deflate".into(),
        n => format!("Method {n}"),
    }
}

fn read_local(r: &mut (impl Read + Seek), src: &Source<'_>) -> Result<Local> {
    let name_len = r.u16()? as usize;
    let attr = r.u8()?;
    let time = r.u32()?;
    let flags = r.u8()?;
    let _ = r.u8()?;
    let width = (flags >> 4) as usize;
    let (method, crc, packed, size) = match width {
        0 => (0, 0, 0, 0),
        1 | 2 | 4 | 8 => {
            let method = r.u8()?;
            let _ = r.u8()?;
            let crc = r.u32()?;
            (method, crc, r.uint(width)?, r.uint(width)?)
        }
        _ => return Err(damaged(&format!("size width {width}"))),
    };
    let raw = r.vec(name_len)?;
    let encrypted = flags & FLAG_ENCRYPTED != 0;
    let encryption_header = if encrypted { Some(r.array()?) } else { None };
    let data = r.stream_position()?;
    let mut item = Item::new(&decode_name(&raw, false, src.encoding), attr & ATTR_DIRECTORY != 0);
    item.size = size;
    item.packed = Some(packed);
    item.modified = dos_time(time);
    item.encrypted = encrypted;
    item.method = method_name(method);
    Ok(Local { item, method, crc, flags, time, encryption_header, data })
}

/// The decoded contents of `e`, checked against its size and CRC on the way out.
fn open<'a>(vols: &'a Volumes<'a>, e: &Local, src: &Source<'_>) -> Result<Verify<Box<dyn Read + 'a>>> {
    let packed = e.item.packed.unwrap_or(0);
    let mut raw: Box<dyn Read + 'a> = Box::new(vols.slice(e.data, packed));
    if let Some(header) = &e.encryption_header {
        let mut keys = Keys::new(&zipcrypto::password_bytes(src.require_password()?));
        let check = if e.flags & FLAG_DATA_DESCRIPTOR != 0 { (e.time >> 8) as u8 } else { (e.crc >> 24) as u8 };
        if keys.decrypt_header(header) != check {
            return Err(ArchiveError::WrongPassword.into());
        }
        raw = Box::new(ZipCryptoReader::new(raw, keys));
    }
    let decoded: Box<dyn Read + 'a> = match e.method {
        0 => raw,
        1 => Box::new(Bzip2Reader::new(raw, Framing::Alz)),
        2 => Box::new(DeflateDecoder::new(raw)),
        m => return Err(ArchiveError::Unsupported(trf!("ALZ 압축 방식 {m}", "ALZ compression method {m}")).into()),
    };
    Ok(Verify::new(decoded, e.item.size, Some(e.crc)))
}

pub fn walk(src: &Source<'_>, v: &mut dyn Visitor) -> Result<()> {
    walk_archive(src, v).map_err(|e| cancel_aware(e, src.input))
}

fn walk_archive(src: &Source<'_>, v: &mut dyn Visitor) -> Result<()> {
    let vols = open_volumes(src)?;
    let mut r = vols.reader(0);
    if r.signature()? != Some(ARCHIVE) {
        bail!(tr!("ALZ 압축 파일이 아닙니다.", "This is not an ALZ archive."));
    }
    r.seek_relative(4)?;
    loop {
        match r.signature()? {
            Some(LOCAL) => {}
            Some(CENTRAL | END) | None => return Ok(()),
            Some(sig) => return Err(damaged(&format!("{sig:08X}"))),
        }
        let e = read_local(&mut r, src)?;
        if v.entry(&e.item)? && !e.item.is_dir {
            let mut data = open(&vols, &e, src)?;
            let res = v.data(&e.item, &mut data);
            if data.failed && e.item.encrypted {
                return Err(ArchiveError::WrongPassword.into());
            }
            res?;
        }
        let next = e.data.checked_add(e.item.packed.unwrap_or(0)).ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?;
        r.seek(io::SeekFrom::Start(next))?;
    }
}
