//! Readers for each archive format that is not ZIP or 7z.

pub mod alz;
pub mod ar;
pub mod arj;
pub mod cab;
pub mod cpio;
pub mod egg;
pub mod iso;
pub mod lzh;
pub mod rar;
pub mod tar;
mod volumes;

#[cfg(test)]
mod alz_egg_tests;
