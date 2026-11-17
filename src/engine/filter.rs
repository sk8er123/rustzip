//! Single-stream compression filters (gzip, bzip2, xz, ...): the outer layer of
//! `.tar.gz`, `.cpio.xz` or a lone `.zst` file.

use std::io::{self, Read, Write};

use anyhow::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Filter {
    None,
    Gzip,
    Bzip2,
    Xz,
    Lzma,
    Lzip,
    Zstd,
    Lz4,
    Brotli,
    Snappy,
    /// Unix `compress` (.Z, LZW).
    Compress,
}

/// A compressing writer that must be told when the data ends.
pub trait FinishWrite: Write {
    fn finish(self: Box<Self>) -> io::Result<()>;
}

macro_rules! finish_with {
    ($t:ty, |$s:ident| $e:expr) => {
        impl<W: Write> FinishWrite for $t {
            fn finish(self: Box<Self>) -> io::Result<()> {
                let $s = *self;
                $e.map(|_| ())
            }
        }
    };
}

finish_with!(flate2::write::GzEncoder<W>, |s| s.finish());
finish_with!(bzip2::write::BzEncoder<W>, |s| s.finish());
finish_with!(lzma_rust2::XzWriter<W>, |s| s.finish().map_err(io::Error::other));
finish_with!(lzma_rust2::LzmaWriter<W>, |s| s.finish().map_err(io::Error::other));
finish_with!(lzma_rust2::LzipWriter<W>, |s| s.finish().map_err(io::Error::other));
finish_with!(zstd::stream::write::Encoder<'static, W>, |s| s.finish());
finish_with!(lz4_flex::frame::FrameEncoder<W>, |s| s.finish().map_err(io::Error::other));
finish_with!(brotli::CompressorWriter<W>, |s| {
    let mut s = s;
    s.flush().map(|_| s.into_inner())
});
finish_with!(snap::write::FrameEncoder<W>, |s| s.into_inner().map_err(|e| io::Error::other(e.to_string())));

struct Plain<W>(W);
impl<W: Write> Write for Plain<W> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0.write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
finish_with!(Plain<W>, |s| { let mut s = s; s.0.flush() });

impl Filter {
    /// Every real filter, in the order shown to people.
    pub const ALL: [Filter; 10] = [
        Filter::Gzip,
        Filter::Bzip2,
        Filter::Xz,
        Filter::Zstd,
        Filter::Lz4,
        Filter::Lzma,
        Filter::Lzip,
        Filter::Brotli,
        Filter::Snappy,
        Filter::Compress,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Filter::None => "",
            Filter::Gzip => "GZ",
            Filter::Bzip2 => "BZ2",
            Filter::Xz => "XZ",
            Filter::Lzma => "LZMA",
            Filter::Lzip => "LZ",
            Filter::Zstd => "ZST",
            Filter::Lz4 => "LZ4",
            Filter::Brotli => "BR",
            Filter::Snappy => "SZ",
            Filter::Compress => "Z",
        }
    }

    /// File extension without the dot.
    pub fn ext(self) -> &'static str {
        match self {
            Filter::None => "",
            Filter::Gzip => "gz",
            Filter::Bzip2 => "bz2",
            Filter::Xz => "xz",
            Filter::Lzma => "lzma",
            Filter::Lzip => "lz",
            Filter::Zstd => "zst",
            Filter::Lz4 => "lz4",
            Filter::Brotli => "br",
            Filter::Snappy => "sz",
            Filter::Compress => "Z",
        }
    }

    /// Name of the algorithm, for the method column and menus.
    pub fn algorithm(self) -> &'static str {
        match self {
            Filter::None => "",
            Filter::Gzip => "Deflate (gzip)",
            Filter::Bzip2 => "BZip2",
            Filter::Xz => "LZMA2 (xz)",
            Filter::Lzma => "LZMA",
            Filter::Lzip => "LZMA (lzip)",
            Filter::Zstd => "Zstandard",
            Filter::Lz4 => "LZ4",
            Filter::Brotli => "Brotli",
            Filter::Snappy => "Snappy",
            Filter::Compress => "LZW (compress)",
        }
    }

    /// Short names that mean "tar inside this filter" (`a.tgz`).
    pub fn tar_shorthands(self) -> &'static [&'static str] {
        match self {
            Filter::Gzip => &["tgz", "taz"],
            Filter::Bzip2 => &["tbz", "tbz2", "tb2"],
            Filter::Xz => &["txz"],
            Filter::Lzma => &["tlz", "tlzma"],
            Filter::Zstd => &["tzst"],
            Filter::Compress => &["taz", "tz"],
            _ => &[],
        }
    }

    pub fn can_write(self) -> bool {
        self != Filter::Compress
    }

    /// Recognises a filter by its first bytes. Brotli has no signature and LZMA's is
    /// weak, so those two also need the file extension (`ext_lower`).
    pub fn sniff(head: &[u8], ext_lower: &str) -> Option<Filter> {
        const SNAPPY: &[u8] = b"\xff\x06\x00\x00sNaPpY";
        let f = if head.starts_with(&[0x1f, 0x8b]) {
            Filter::Gzip
        } else if head.starts_with(b"BZh") && head.get(3).is_some_and(|b| (b'1'..=b'9').contains(b)) {
            Filter::Bzip2
        } else if head.starts_with(&[0xFD, b'7', b'z', b'X', b'Z', 0x00]) {
            Filter::Xz
        } else if head.starts_with(b"LZIP") {
            Filter::Lzip
        } else if head.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
            Filter::Zstd
        } else if head.starts_with(&[0x04, 0x22, 0x4D, 0x18]) {
            Filter::Lz4
        } else if head.starts_with(SNAPPY) {
            Filter::Snappy
        } else if head.starts_with(&[0x1f, 0x9d]) {
            Filter::Compress
        } else if (ext_lower == "lzma" || ext_lower == "tlz") && head.first().is_some_and(|&p| p < 225) && head.len() >= 13 {
            Filter::Lzma
        } else if ext_lower == "br" {
            Filter::Brotli
        } else {
            return None;
        };
        Some(f)
    }

    /// Wraps `r` in this filter's decompressor.
    pub fn reader<'a>(self, r: impl Read + 'a) -> Result<Box<dyn Read + 'a>> {
        let r = io::BufReader::new(r);
        Ok(match self {
            Filter::None => Box::new(r),
            Filter::Gzip => Box::new(flate2::read::MultiGzDecoder::new(r)),
            Filter::Bzip2 => Box::new(bzip2::read::MultiBzDecoder::new(r)),
            Filter::Xz => Box::new(lzma_rust2::XzReader::new(r, true)),
            Filter::Lzma => Box::new(lzma_rust2::LzmaReader::new_mem_limit(r, u32::MAX, None)?),
            Filter::Lzip => Box::new(lzma_rust2::LzipReader::new(r)),
            Filter::Zstd => Box::new(zstd::stream::read::Decoder::with_buffer(r)?),
            Filter::Lz4 => Box::new(lz4_flex::frame::FrameDecoder::new(r)),
            Filter::Brotli => Box::new(brotli::Decompressor::new(r, 64 * 1024)),
            Filter::Snappy => Box::new(snap::read::FrameDecoder::new(r)),
            Filter::Compress => Box::new(super::lzw::LzwReader::new(r)?),
        })
    }

    /// Wraps `w` in this filter's compressor. `level` is 0 (fastest) ..= 9 (smallest).
    pub fn writer<'a>(self, w: impl Write + 'a, level: u8) -> Result<Box<dyn FinishWrite + 'a>> {
        let level = level.min(9) as u32;
        Ok(match self {
            Filter::None => Box::new(Plain(w)),
            Filter::Gzip => Box::new(flate2::write::GzEncoder::new(w, flate2::Compression::new(level))),
            Filter::Bzip2 => Box::new(bzip2::write::BzEncoder::new(w, bzip2::Compression::new(level.max(1)))),
            Filter::Xz => Box::new(lzma_rust2::XzWriter::new(w, lzma_rust2::XzOptions::with_preset(level))?),
            Filter::Lzma => Box::new(lzma_rust2::LzmaWriter::new_use_header(w, &lzma_rust2::LzmaOptions::with_preset(level), None)?),
            Filter::Lzip => Box::new(lzma_rust2::LzipWriter::new(w, lzma_rust2::LzipOptions::with_preset(level))),
            // zstd 1..=19 (19 = "ultra" without the huge window).
            Filter::Zstd => Box::new(zstd::stream::write::Encoder::new(w, [1, 1, 2, 3, 5, 7, 9, 12, 15, 19][level as usize])?),
            Filter::Lz4 => Box::new(lz4_flex::frame::FrameEncoder::new(w)),
            // brotli quality 0..=11, window 22 (4 MiB).
            Filter::Brotli => Box::new(brotli::CompressorWriter::new(w, 64 * 1024, [0, 1, 2, 4, 5, 6, 8, 9, 10, 11][level as usize], 22)),
            Filter::Snappy => Box::new(snap::write::FrameEncoder::new(w)),
            Filter::Compress => {
                return Err(super::ArchiveError::Unsupported(tr!(".Z 형식으로는 압축할 수 없습니다", "cannot compress to .Z").into()).into())
            }
        })
    }
}
