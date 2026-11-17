//! Unix `ar` archives (.a, .deb), GNU and BSD long names.

use std::io::Read;

use anyhow::{bail, Result};

use super::super::walk::{drain, Item, Source, Visitor};
use super::super::{decode_name, unix_to_naive};

pub fn walk(src: &Source<'_>, v: &mut dyn Visitor) -> Result<()> {
    let mut r = src.open_stream()?;
    let mut magic = [0u8; 8];
    r.read_exact(&mut magic)?;
    if &magic != b"!<arch>\n" {
        bail!("AR: bad signature");
    }
    let mut long_names: Vec<u8> = Vec::new();
    loop {
        let mut h = [0u8; 60];
        match r.read(&mut h[..1])? {
            0 => return Ok(()),
            _ if h[0] == b'\n' => continue, // padding byte after odd-sized data
            _ => r.read_exact(&mut h[1..])?,
        }
        let field = |a: usize, b: usize| String::from_utf8_lossy(&h[a..b]).trim().to_string();
        let size: u64 = field(48, 58).parse()?;
        let mtime: i64 = field(16, 28).parse().unwrap_or(0);
        let mut data = (&mut r).take(size);
        let name: Vec<u8> = if field(0, 16) == "//" {
            long_names.clear();
            data.read_to_end(&mut long_names)?;
            continue;
        } else if field(0, 16) == "/" || field(0, 16).starts_with("__.SYMDEF") {
            drain(&mut data)?;
            continue;
        } else if let Some(n) = field(0, 16).strip_prefix("#1/") {
            let mut name = vec![0u8; n.parse()?];
            data.read_exact(&mut name)?;
            while name.last() == Some(&0) {
                name.pop();
            }
            name
        } else if let Some(off) = field(0, 16).strip_prefix('/').and_then(|o| o.parse::<usize>().ok()) {
            let rest = long_names.get(off..).unwrap_or_default();
            let end = rest.iter().position(|&b| b == b'\n').unwrap_or(rest.len());
            rest[..end].strip_suffix(b"/").unwrap_or(&rest[..end]).to_vec()
        } else {
            let n = field(0, 16);
            n.strip_suffix('/').unwrap_or(&n).as_bytes().to_vec()
        };
        let mut item = Item::new(&decode_name(&name, false, src.encoding), false);
        item.size = data.limit();
        item.modified = unix_to_naive(mtime);
        item.method = src.stream_method("AR");
        if v.entry(&item)? {
            v.data(&item, &mut data)?;
        }
        drain(&mut data)?;
    }
}
