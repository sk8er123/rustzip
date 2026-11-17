//! RAR 1.5-7 through the official UnRAR library.

use std::path::Path;

use anyhow::Result;
use unrar::error::{Code, UnrarError};

use super::super::{dos_to_naive, normalize_entry_path, ArchiveError, Entry, Listing, Planner, Progress};

fn rar_err(e: UnrarError, entry_encrypted: bool) -> anyhow::Error {
    match e.code {
        Code::MissingPassword => ArchiveError::PasswordRequired.into(),
        Code::BadPassword => ArchiveError::WrongPassword.into(),
        // RAR 4 cannot tell a wrong password from damage; CRC errors on encrypted data mean the former.
        Code::BadData if entry_encrypted => ArchiveError::WrongPassword.into(),
        _ => anyhow::Error::new(e),
    }
}

fn archive<'a>(path: &'a Path, password: Option<&'a str>) -> unrar::Archive<'a> {
    let a = match password.filter(|p| !p.is_empty()) {
        Some(pw) => unrar::Archive::with_password(path, pw.as_bytes()),
        None => unrar::Archive::new(path),
    };
    if a.is_multipart() {
        a.as_first_part()
    } else {
        a
    }
}

fn method_label(m: u32) -> String {
    match m {
        0x30 => tr!("저장", "Store").into(),
        0x31..=0x35 => format!("RAR m{}", m - 0x30),
        _ => "RAR".into(),
    }
}

pub fn list(path: &Path, password: Option<&str>) -> Result<Vec<Entry>> {
    let mut out = Vec::new();
    for h in archive(path, password).open_for_listing().map_err(|e| rar_err(e, false))? {
        let h = h.map_err(|e| rar_err(e, false))?;
        let p = normalize_entry_path(&h.filename.to_string_lossy());
        if p.is_empty() {
            continue;
        }
        out.push(Entry {
            index: out.len(),
            path: p,
            is_dir: h.is_directory(),
            size: if h.is_directory() { 0 } else { h.unpacked_size },
            packed: None,
            modified: dos_to_naive(h.file_time),
            encrypted: h.is_encrypted(),
            method: method_label(h.method),
        });
    }
    Ok(out)
}

pub fn extract(listing: &Listing, planner: &mut Planner<'_>, progress: &Progress) -> Result<()> {
    let password = planner.opts.password.clone();
    let mut cursor = archive(&listing.path, password.as_deref()).open_for_processing().map_err(|e| rar_err(e, false))?;
    while let Some(h) = cursor.read_header().map_err(|e| rar_err(e, false))? {
        progress.check()?;
        let entry = h.entry();
        let path = normalize_entry_path(&entry.filename.to_string_lossy());
        let (is_dir, size, encrypted, mtime) = (entry.is_directory(), entry.unpacked_size, entry.is_encrypted(), dos_to_naive(entry.file_time));
        progress.set_current(&path);
        let target = if path.is_empty() { None } else { planner.target(&path, is_dir) };
        cursor = match target {
            Some(out) if is_dir => {
                planner.make_dir(&out)?;
                h.skip().map_err(|e| rar_err(e, false))?
            }
            Some(out) => {
                if encrypted && password.as_deref().is_none_or(str::is_empty) {
                    return Err(ArchiveError::PasswordRequired.into());
                }
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let next = h.extract_to(&out).map_err(|e| {
                    let _ = std::fs::remove_file(&out);
                    super::super::entry_error(rar_err(e, encrypted), &path)
                })?;
                progress.add(size);
                planner.finish_file(out, size, mtime);
                next
            }
            None => {
                if !is_dir {
                    progress.add(size);
                }
                h.skip().map_err(|e| rar_err(e, false))?
            }
        };
    }
    Ok(())
}
