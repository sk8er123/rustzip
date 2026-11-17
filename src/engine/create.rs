//! Writers for the non-ZIP formats RustZip can create: 7z, tar (in any filter)
//! and single compressed files.

use std::fs::File;
use std::io::{self, BufWriter, Read, Write};

use anyhow::{bail, Result};
use sevenz_rust2::encoder_options::{
    AesEncoderOptions, BrotliOptions, Bzip2Options, DeflateOptions, Lz4Options, Lzma2Options, LzmaOptions, PpmdOptions, ZstandardOptions,
};
use sevenz_rust2::{ArchiveEntry, ArchiveWriter, EncoderConfiguration, EncoderMethod};

use super::{ArchiveError, Filter, Method, PackOptions, Progress, SourceItem};

/// Reads a source file while moving the progress bar and honouring cancel.
struct ProgressReader<'a> {
    inner: File,
    progress: &'a Progress,
}

impl Read for ProgressReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.progress.cancelled() {
            // Not `Interrupted`: read_exact and io::copy retry that kind forever.
            return Err(io::Error::other(ArchiveError::Cancelled));
        }
        let n = self.inner.read(buf)?;
        self.progress.add(n as u64);
        Ok(n)
    }
}

fn open_source<'a>(item: &SourceItem, progress: &'a Progress) -> Result<ProgressReader<'a>> {
    progress.set_current(&item.name);
    let f = File::open(&item.disk).map_err(|e| anyhow::Error::new(e).context(trf!("읽을 수 없습니다: {}", "Cannot read: {}", item.disk.display())))?;
    Ok(ProgressReader { inner: f, progress })
}

fn cancelled_or(err: anyhow::Error, progress: &Progress) -> anyhow::Error {
    if progress.cancelled() {
        ArchiveError::Cancelled.into()
    } else {
        err
    }
}

fn sevenz_method(method: Method, level: u8) -> EncoderConfiguration {
    let l = level.min(9) as u32;
    if level == 0 {
        return EncoderMethod::COPY.into();
    }
    match method {
        Method::Store => EncoderMethod::COPY.into(),
        Method::Lzma => LzmaOptions::from_level(l).into(),
        Method::Ppmd => PpmdOptions::from_level(l).into(),
        Method::Bzip2 => Bzip2Options::from_level(l).into(),
        Method::Deflate => DeflateOptions::from_level(l).into(),
        Method::Zstd => ZstandardOptions::from_level([1, 1, 2, 3, 5, 7, 9, 12, 15, 19][l as usize]).into(),
        Method::Brotli => BrotliOptions::from_quality_window([0, 1, 2, 4, 5, 6, 8, 9, 10, 11][l as usize], 22).into(),
        Method::Lz4 => Lz4Options::default().into(),
        Method::Lzma2 | Method::Xz => Lzma2Options::from_level(l).into(),
    }
}

pub fn sevenz(out: &File, items: &[SourceItem], opts: &PackOptions, progress: &Progress) -> Result<()> {
    let mut w = ArchiveWriter::new(BufWriter::new(out))?;
    let mut methods = Vec::new();
    if let Some(pw) = opts.password.as_deref().filter(|p| !p.is_empty()) {
        if !super::ArchiveKind::SEVENZ.methods_for(true).contains(&opts.method) {
            bail!(trf!("7Z에서 {} 방식에는 암호를 걸 수 없습니다.", "7z cannot combine {} with a password.", opts.method.label()));
        }
        methods.push(AesEncoderOptions::new(pw.into()).into());
        w.set_encrypt_header(true);
    }
    methods.push(sevenz_method(opts.method, opts.level));
    w.set_content_methods(methods);
    for item in items {
        progress.check()?;
        let entry = ArchiveEntry::from_path(&item.disk, item.name.clone());
        if item.is_dir {
            w.push_archive_entry::<File>(entry, None)?;
        } else {
            let r = open_source(item, progress)?;
            w.push_archive_entry(entry, Some(r)).map_err(|e| cancelled_or(e.into(), progress))?;
        }
    }
    w.finish()?.flush()?;
    Ok(())
}

pub fn tar(out: &File, items: &[SourceItem], filter: Filter, level: u8, progress: &Progress) -> Result<()> {
    let mut b = tar::Builder::new(filter.writer(BufWriter::new(out), level)?);
    b.mode(tar::HeaderMode::Complete);
    for item in items {
        progress.check()?;
        if item.is_dir {
            b.append_dir(&item.name, &item.disk)?;
            continue;
        }
        let meta = std::fs::metadata(&item.disk)?;
        let mut h = tar::Header::new_gnu();
        h.set_metadata_in_mode(&meta, tar::HeaderMode::Complete);
        h.set_size(meta.len());
        let r = open_source(item, progress)?;
        b.append_data(&mut h, &item.name, r).map_err(|e| cancelled_or(e.into(), progress))?;
    }
    b.into_inner()?.finish()?;
    Ok(())
}

pub fn single(out: &File, items: &[SourceItem], filter: Filter, level: u8, progress: &Progress) -> Result<()> {
    let files: Vec<&SourceItem> = items.iter().filter(|i| !i.is_dir).collect();
    if files.len() != 1 || items.len() != 1 {
        bail!(trf!(
            "{} 형식은 파일 하나만 압축할 수 있습니다. 여러 파일은 TAR.{} 형식을 쓰세요.",
            "{} can hold only one file. Use TAR.{} for several files.",
            filter.label(),
            filter.label()
        ));
    }
    let mut w = filter.writer(BufWriter::new(out), level)?;
    let mut r = open_source(files[0], progress)?;
    io::copy(&mut r, &mut w).map_err(|e| cancelled_or(e.into(), progress))?;
    w.finish()?;
    Ok(())
}
