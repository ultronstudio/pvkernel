#![no_std]

//! Testable architecture-neutral path, volume, VFS, and CPIO primitives.

#[cfg(test)]
#[macro_use]
extern crate std;

#[path = "../../src/fs/initrd/cpio.rs"]
pub mod cpio;
pub mod initrd;
#[path = "../../src/fs/path.rs"]
pub mod path;
pub mod vfs;
