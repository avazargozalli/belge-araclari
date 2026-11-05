//! Shared core of Belge Araçları, used by the Windows app as a library and bundled into the macOS
//! app as the `belge-core` command-line tool. Everything runs locally.

/// Copies every field of `$other` that is set onto `$self`.
macro_rules! overlay {
    ($self:ident, $other:ident, $($f:ident),*) => { $( if $other.$f.is_some() { $self.$f = $other.$f.clone(); } )* };
}

pub mod convert;
pub mod office;
pub mod pdf;
pub mod udf;
pub mod udfout;
pub mod word;

#[cfg(test)]
mod tests;
