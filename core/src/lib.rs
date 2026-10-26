//! Shared core of Belge Araçları, used by the Windows app as a library and bundled into the macOS
//! app as the `belge-core` command-line tool. Everything runs locally.

pub mod office;
pub mod udf;
pub mod word;

#[cfg(test)]
mod tests;
