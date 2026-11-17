//! TAR (plain or inside any filter).

use anyhow::Result;

use super::super::walk::{Item, Source, Visitor};
use super::super::{decode_name, unix_to_naive};

pub fn walk(src: &Source<'_>, v: &mut dyn Visitor) -> Result<()> {
    let mut ar = tar::Archive::new(src.open_stream()?);
    for entry in ar.entries()? {
        let mut entry = entry?;
        let et = entry.header().entry_type();
        if !(et.is_file() || et.is_dir() || et == tar::EntryType::GNUSparse) {
            continue; // links, pax headers...
        }
        let mut item = Item::new(&decode_name(&entry.path_bytes(), false, src.encoding), et.is_dir());
        item.size = if et.is_dir() { 0 } else { entry.header().size().unwrap_or(0) };
        item.modified = entry.header().mtime().ok().and_then(|m| unix_to_naive(m as i64));
        item.method = src.stream_method("TAR");
        if v.entry(&item)? {
            v.data(&item, &mut entry)?;
        }
    }
    Ok(())
}
