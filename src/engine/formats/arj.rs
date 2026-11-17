//! ARJ (methods 0-4).

use std::fs::File;
use std::io::BufReader;

use anyhow::{anyhow, Result};
use unarj_rs::arj_archive::ArjArchieve;
use unarj_rs::local_file_header::{CompressionMethod, FileType};

use super::super::walk::{Item, Source, Visitor};
use super::super::{decode_name, ArchiveError};

pub fn walk(src: &Source<'_>, v: &mut dyn Visitor) -> Result<()> {
    let file = File::open(src.path)?;
    let mut arj = ArjArchieve::new(BufReader::new(file)).map_err(|e| anyhow!("ARJ: {e}"))?;
    while let Some(h) = arj.get_next_entry()? {
        let is_dir = h.file_type == FileType::Directory;
        // unarj-rs maps each name byte to one char; undo that to decode the real code page.
        let raw: Vec<u8> = h.name.chars().map(|c| c as u32 as u8).collect();
        let mut item = Item::new(&decode_name(&raw, false, src.encoding), is_dir);
        item.size = h.original_size as u64;
        item.packed = Some(h.compressed_size as u64);
        let t = &h.date_time_modified;
        item.modified = chrono::NaiveDate::from_ymd_opt(t.year() as i32, t.month() as u32, t.day() as u32)
            .and_then(|d| d.and_hms_opt(t.hour() as u32, t.minute() as u32, t.second() as u32));
        item.encrypted = h.is_garbled();
        item.method = match h.compression_method {
            CompressionMethod::Stored => tr!("저장", "Store").into(),
            m => format!("{m:?}"),
        };
        let wanted = matches!(h.file_type, FileType::Binary | FileType::Text7Bit | FileType::Directory) && v.entry(&item)?;
        if wanted && !is_dir {
            if item.encrypted {
                return Err(ArchiveError::Unsupported(tr!("암호가 걸린 ARJ", "password-protected ARJ").into()).into());
            }
            let data = arj.read(&h)?;
            v.data(&item, &mut data.as_slice())?;
        } else {
            arj.skip(&h)?;
        }
    }
    Ok(())
}
