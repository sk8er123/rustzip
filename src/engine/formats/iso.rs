//! ISO 9660 CD/DVD images, preferring the Joliet (Unicode) names when present.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};

use anyhow::{bail, Result};
use chrono::{NaiveDate, NaiveDateTime};

use super::super::walk::{Item, Source, Visitor};
use super::super::decode_name;

const SECTOR: u64 = 2048;

#[derive(Debug, Clone)]
struct Record {
    extent: u64,
    len: u64,
    is_dir: bool,
    name: Vec<u8>,
    modified: Option<NaiveDateTime>,
}

fn parse_record(b: &[u8]) -> Option<Record> {
    let len = *b.first()? as usize;
    if len < 34 || b.len() < len {
        return None;
    }
    let name_len = b[32] as usize;
    let name = b.get(33..33 + name_len)?.to_vec();
    let t = &b[18..25];
    let modified = NaiveDate::from_ymd_opt(1900 + t[0] as i32, t[1] as u32, t[2] as u32)
        .and_then(|d| d.and_hms_opt(t[3] as u32, t[4] as u32, t[5] as u32));
    Some(Record {
        extent: u32::from_le_bytes(b[2..6].try_into().ok()?) as u64,
        len: u32::from_le_bytes(b[10..14].try_into().ok()?) as u64,
        is_dir: b[25] & 0x02 != 0,
        name,
        modified,
    })
}

struct Image {
    f: BufReader<File>,
    joliet: bool,
}

impl Image {
    fn read_at(&mut self, pos: u64, len: usize) -> Result<Vec<u8>> {
        self.f.seek(SeekFrom::Start(pos))?;
        let mut buf = vec![0u8; len];
        self.f.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// Finds the root directory, from the Joliet descriptor when there is one.
    fn root(&mut self) -> Result<Record> {
        let mut primary = None;
        for i in 16..64 {
            let d = self.read_at(i * SECTOR, SECTOR as usize)?;
            if &d[1..6] != b"CD001" {
                bail!("ISO: bad volume descriptor");
            }
            match d[0] {
                1 if primary.is_none() => primary = parse_record(&d[156..190]),
                2 if d[88] == b'%' && d[89] == b'/' && matches!(d[90], b'@' | b'C' | b'E') => {
                    if let Some(r) = parse_record(&d[156..190]) {
                        self.joliet = true;
                        return Ok(r);
                    }
                }
                255 => break,
                _ => {}
            }
        }
        primary.ok_or_else(|| anyhow::anyhow!("ISO: no primary volume descriptor"))
    }

    fn name(&self, r: &Record, encoding: super::super::NameEncoding) -> String {
        let s = if self.joliet {
            let units: Vec<u16> = r.name.as_chunks::<2>().0.iter().map(|&c| u16::from_be_bytes(c)).collect();
            String::from_utf16_lossy(&units)
        } else {
            decode_name(&r.name, false, encoding)
        };
        // "NAME.TXT;1" → "NAME.TXT", "README." → "README"
        let s = match s.rsplit_once(';') {
            Some((base, ver)) if ver.chars().all(|c| c.is_ascii_digit()) => base.to_string(),
            _ => s,
        };
        if !r.is_dir {
            if let Some(stripped) = s.strip_suffix('.') {
                return stripped.to_string();
            }
        }
        s
    }

    fn children(&mut self, dir: &Record) -> Result<Vec<Record>> {
        let data = self.read_at(dir.extent * SECTOR, dir.len.min(64 << 20) as usize)?;
        let mut out = Vec::new();
        let mut pos = 0usize;
        while pos < data.len() {
            let len = data[pos] as usize;
            if len == 0 {
                // Records never cross a sector; zero means "continue in the next one".
                pos = (pos / SECTOR as usize + 1) * SECTOR as usize;
                continue;
            }
            let Some(r) = parse_record(&data[pos..]) else { break };
            pos += len;
            if r.name == [0] || r.name == [1] {
                continue; // "." and ".."
            }
            out.push(r);
        }
        Ok(out)
    }
}

pub fn walk(src: &Source<'_>, v: &mut dyn Visitor) -> Result<()> {
    let mut img = Image { f: BufReader::new(File::open(src.path)?), joliet: false };
    let root = img.root()?;
    let mut stack = vec![(String::new(), root, 0u32)];
    while let Some((prefix, dir, depth)) = stack.pop() {
        if depth > 64 {
            bail!("ISO: directories nested too deep");
        }
        let mut subdirs = Vec::new();
        for r in img.children(&dir)? {
            let name = img.name(&r, src.encoding);
            let path = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            let mut item = Item::new(&path, r.is_dir);
            item.size = if r.is_dir { 0 } else { r.len };
            item.packed = (!r.is_dir).then_some(r.len);
            item.modified = r.modified;
            item.method = "ISO".into();
            if v.entry(&item)? && !r.is_dir {
                img.f.seek(SeekFrom::Start(r.extent * SECTOR))?;
                v.data(&item, &mut (&mut img.f).take(r.len))?;
            }
            if r.is_dir {
                subdirs.push((item.path, r, depth + 1));
            }
        }
        stack.extend(subdirs.into_iter().rev());
    }
    Ok(())
}
