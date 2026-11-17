//! Microsoft Cabinet (stored, MSZIP, LZX).

use std::fs::File;
use std::io::BufReader;

use anyhow::Result;

use super::super::walk::{Item, Source, Visitor};

pub fn walk(src: &Source<'_>, v: &mut dyn Visitor) -> Result<()> {
    let mut cab = cab::Cabinet::new(BufReader::new(File::open(src.path)?))?;
    let mut items = Vec::new();
    for folder in cab.folder_entries() {
        let method = match folder.compression_type() {
            cab::CompressionType::None => tr!("저장", "Store").to_string(),
            cab::CompressionType::MsZip => "MSZIP".to_string(),
            cab::CompressionType::Quantum(..) => "Quantum".to_string(),
            cab::CompressionType::Lzx(_) => "LZX".to_string(),
        };
        for f in folder.file_entries() {
            let mut item = Item::new(f.name(), false);
            item.size = f.uncompressed_size() as u64;
            item.modified = f.datetime().and_then(|t| {
                chrono::NaiveDate::from_ymd_opt(t.year(), t.month() as u32, t.day() as u32)
                    .and_then(|d| d.and_hms_opt(t.hour() as u32, t.minute() as u32, t.second() as u32))
            });
            item.method = method.clone();
            items.push((f.name().to_string(), item));
        }
    }
    for (name, item) in items {
        if v.entry(&item)? {
            let mut r = cab.read_file(&name)?;
            v.data(&item, &mut r)?;
        }
    }
    Ok(())
}
