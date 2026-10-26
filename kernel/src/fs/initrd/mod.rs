//! Read-only CPIO `newc` initrd provider for the temporary `Boot:` volume.

pub use pvkernel_fs_core::cpio::Archive;
#[cfg(not(feature = "initrd-missing-volume-test"))]
pub use pvkernel_fs_core::initrd::InitrdFs;
