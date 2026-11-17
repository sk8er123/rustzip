//! One shape for every archive that is read entry by entry (tar, cpio, ar, lzh, arj,
//! cab, iso, alz, egg): a walker reports each entry to a [`Visitor`] and hands over
//! the contents only when asked. Listing and extracting are two visitors.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

use anyhow::Result;
use chrono::NaiveDateTime;

use super::{copy_with_progress, entry_error, normalize_entry_path, ArchiveError, Entry, Filter, NameEncoding, Planner, Progress};

/// What a walker knows about one entry before reading its contents.
#[derive(Debug, Clone, Default)]
pub struct Item {
    /// Normalized `a/b/c` path.
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub packed: Option<u64>,
    pub modified: Option<NaiveDateTime>,
    pub encrypted: bool,
    pub method: String,
}

impl Item {
    pub fn new(raw_path: &str, is_dir: bool) -> Self {
        Self { path: normalize_entry_path(raw_path), is_dir, ..Default::default() }
    }
}

/// Everything a walker needs to read one archive.
pub struct Source<'a> {
    pub path: &'a Path,
    /// Outer compression (only tar, cpio and ar can have one).
    pub filter: Filter,
    pub encoding: NameEncoding,
    pub password: Option<&'a str>,
    /// Gets the number of archive bytes read, for listing progress.
    pub input: &'a Progress,
}

impl Source<'_> {
    pub fn password(&self) -> Option<&str> {
        self.password.filter(|p| !p.is_empty())
    }

    /// The password for an encrypted entry, or [`ArchiveError::PasswordRequired`].
    pub fn require_password(&self) -> Result<&str> {
        self.password().ok_or_else(|| ArchiveError::PasswordRequired.into())
    }

    /// Method column for entries of a stream container: the filter's algorithm, if any.
    pub fn stream_method(&self, container: &str) -> String {
        match self.filter {
            Filter::None => container.to_string(),
            f => f.algorithm().to_string(),
        }
    }

    /// The archive file, decompressed by `filter`, counting raw bytes into `input`.
    pub fn open_stream(&self) -> Result<Box<dyn Read + '_>> {
        let f = File::open(self.path)?;
        self.input.set_total(f.metadata()?.len());
        self.filter.reader(CountingReader { inner: f, progress: self.input })
    }
}

pub trait Visitor {
    /// Called for every entry in archive order. Return true to get its contents.
    fn entry(&mut self, item: &Item) -> Result<bool>;
    /// Called right after `entry` returned true. Contents not read are skipped.
    fn data(&mut self, item: &Item, data: &mut dyn Read) -> Result<()>;
}

pub type Walker = fn(&Source<'_>, &mut dyn Visitor) -> Result<()>;

/// Counts bytes read from the archive file and stops when cancelled.
pub struct CountingReader<'a, R> {
    pub inner: R,
    pub progress: &'a Progress,
}

impl<R: Read> Read for CountingReader<'_, R> {
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

/// Turns a cancelled read back into [`ArchiveError::Cancelled`].
pub fn cancel_aware(err: impl Into<anyhow::Error>, progress: &Progress) -> anyhow::Error {
    if progress.cancelled() {
        ArchiveError::Cancelled.into()
    } else {
        err.into()
    }
}

struct Lister<'a> {
    out: Vec<Entry>,
    progress: &'a Progress,
}

impl Visitor for Lister<'_> {
    fn entry(&mut self, item: &Item) -> Result<bool> {
        self.progress.check()?;
        if !item.path.is_empty() {
            self.out.push(Entry {
                index: self.out.len(),
                path: item.path.clone(),
                is_dir: item.is_dir,
                size: if item.is_dir { 0 } else { item.size },
                packed: item.packed,
                modified: item.modified,
                encrypted: item.encrypted,
                method: item.method.clone(),
            });
        }
        Ok(false)
    }
    fn data(&mut self, _: &Item, _: &mut dyn Read) -> Result<()> {
        Ok(())
    }
}

pub fn list(walker: Walker, src: &Source<'_>) -> Result<Vec<Entry>> {
    let mut v = Lister { out: Vec::new(), progress: src.input };
    walker(src, &mut v).map_err(|e| cancel_aware(e, src.input))?;
    Ok(v.out)
}

struct Extractor<'a, 'b> {
    planner: &'a mut Planner<'b>,
    progress: &'a Progress,
    target: Option<std::path::PathBuf>,
}

impl Visitor for Extractor<'_, '_> {
    fn entry(&mut self, item: &Item) -> Result<bool> {
        self.progress.check()?;
        if item.path.is_empty() {
            return Ok(false);
        }
        self.progress.set_current(&item.path);
        let Some(out) = self.planner.target(&item.path, item.is_dir) else {
            if !item.is_dir {
                self.progress.add(item.size);
            }
            return Ok(false);
        };
        if item.is_dir {
            self.planner.make_dir(&out)?;
            return Ok(false);
        }
        self.target = Some(out);
        Ok(true)
    }

    fn data(&mut self, item: &Item, data: &mut dyn Read) -> Result<()> {
        let Some(out) = self.target.take() else { return Ok(()) };
        let mut f = self.planner.create_file(&out)?;
        let res = copy_with_progress(data, &mut f, self.progress);
        drop(f);
        match res {
            Ok(n) => {
                self.planner.finish_file(out, n, item.modified);
                Ok(())
            }
            Err(e) => {
                let _ = fs::remove_file(&out);
                Err(entry_error(e, &item.path))
            }
        }
    }
}

pub fn extract(walker: Walker, src: &Source<'_>, planner: &mut Planner<'_>, progress: &Progress) -> Result<()> {
    let mut v = Extractor { planner, progress, target: None };
    walker(src, &mut v)
}

/// Reads and throws away whatever is left of `r` (for formats without seeking).
pub fn drain(r: &mut dyn Read) -> io::Result<()> {
    io::copy(r, &mut io::sink()).map(|_| ())
}
