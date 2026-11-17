//! LZH / LHA (lh0-lh7, lhx, lz4/lz5/lzs, pm0-pm2).

use std::fs::File;
use std::io::{BufReader, Read};

use anyhow::{anyhow, bail, Result};
use chrono::Local;
use delharc::header::TimestampResult;

use super::super::walk::{drain, CountingReader, Item, Source, Visitor};
use super::super::{decode_name, ArchiveError};

fn lha_err<E: std::fmt::Display>(e: E) -> anyhow::Error {
    anyhow!("LZH: {e}")
}

fn item_for(h: &delharc::LhaHeader, src: &Source<'_>) -> Item {
    let mut path = h.parse_pathname_to_str();
    // delharc decodes names as UTF-8; old archives use the local code page (CP949, Shift-JIS).
    if std::str::from_utf8(&h.filename).is_err() {
        let raw: Vec<u8> = h.filename.iter().map(|&b| if b == 0xFF { b'/' } else { b }).collect();
        let decoded = decode_name(&raw, false, src.encoding);
        path = match path.rsplit_once('/') {
            Some((dir, _)) if !h.is_directory() && !raw.contains(&b'/') && !raw.contains(&b'\\') => format!("{dir}/{decoded}"),
            _ => decoded,
        };
    }
    let mut item = Item::new(&path, h.is_directory());
    item.size = h.original_size;
    item.packed = Some(h.compressed_size);
    item.modified = match h.parse_last_modified() {
        TimestampResult::Naive(n) => Some(n),
        TimestampResult::Utc(u) => Some(u.with_timezone(&Local).naive_local()),
        TimestampResult::None => None,
    };
    item.method = String::from_utf8_lossy(&h.compression).trim_matches('-').to_string();
    item
}

pub fn walk(src: &Source<'_>, v: &mut dyn Visitor) -> Result<()> {
    let file = File::open(src.path)?;
    src.input.set_total(file.metadata()?.len());
    let mut lha = delharc::LhaDecodeReader::new(BufReader::new(CountingReader { inner: file, progress: src.input })).map_err(lha_err)?;
    loop {
        let item = item_for(lha.header(), src);
        if v.entry(&item)? && !item.is_dir {
            if !lha.is_decoder_supported() {
                bail!(ArchiveError::Unsupported(format!("LZH -{}-", item.method)));
            }
            v.data(&item, &mut lha as &mut dyn Read)?;
            drain(&mut lha)?;
            lha.crc_check().map_err(lha_err)?;
        }
        if !lha.next_file().map_err(lha_err)? {
            break;
        }
    }
    Ok(())
}
