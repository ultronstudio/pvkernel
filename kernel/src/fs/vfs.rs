//! Boot-volume registration and process-loading adapter over the generic VFS.

use pvkernel_fs_core::vfs::Vfs;
use spin::Once;

#[cfg(not(feature = "initrd-missing-volume-test"))]
use crate::fs::initrd::{Archive, InitrdFs};

pub use pvkernel_fs_core::vfs::{FileKind, FsError, Node};

#[cfg(not(feature = "initrd-missing-volume-test"))]
static BOOT_PROVIDER: Once<InitrdFs<'static>> = Once::new();
static VFS: Once<Vfs<'static>> = Once::new();

#[cfg(not(feature = "initrd-missing-volume-test"))]
pub fn register_boot_volume(archive: Archive<'static>) -> Result<(), FsError> {
    if VFS.get().is_some() {
        return Err(FsError::DuplicateVolume);
    }
    BOOT_PROVIDER.call_once(|| InitrdFs::new(archive));
    let provider = BOOT_PROVIDER.get().expect("boot provider published");
    let mut vfs = Vfs::new();
    vfs.register_volume("Boot", provider)?;
    VFS.call_once(|| vfs);
    Ok(())
}

pub fn volume_count() -> usize {
    VFS.get().map_or(0, Vfs::volume_count)
}

pub fn resolve(path: &str) -> Result<Node<'static>, FsError> {
    VFS.get().ok_or(FsError::VolumeNotFound)?.resolve(path)
}

/// Confirms the boot-time volume registry was published before process load.
pub fn run_boot_checks() -> Result<(), FsError> {
    if volume_count() != 1 {
        return Err(if volume_count() == 0 {
            FsError::VolumeNotFound
        } else {
            FsError::VolumeTableFull
        });
    }
    let boot = resolve("Boot:\\")?;
    if boot.kind() != FileKind::Directory {
        return Err(FsError::NotDirectory);
    }
    Ok(())
}

/// Resolve an archive-backed regular file, then delegate ELF and process
/// policy to the process subsystem.
#[cfg(any(
    not(any(feature = "qemu-test", feature = "userspace-test")),
    feature = "initrd-boot-test",
    feature = "initrd-missing-init-test",
    feature = "initrd-invalid-init-test",
    feature = "initrd-wx-init-test"
))]
pub fn spawn_path(
    parent: Option<crate::process::ProcessId>,
    path: &str,
) -> Result<crate::process::ProcessId, SpawnPathError> {
    let node = resolve(path).map_err(SpawnPathError::Filesystem)?;
    if node.kind() != FileKind::Regular {
        return Err(SpawnPathError::NotRegular);
    }
    let name = node
        .path()
        .rsplit('\\')
        .next()
        .ok_or(SpawnPathError::InvalidName)?;
    let bytes = node
        .contiguous_bytes()
        .map_err(SpawnPathError::Filesystem)?;
    crate::process::spawn_elf(parent, name, bytes).map_err(SpawnPathError::Process)
}

#[cfg(any(
    not(any(feature = "qemu-test", feature = "userspace-test")),
    feature = "initrd-boot-test",
    feature = "initrd-missing-init-test",
    feature = "initrd-invalid-init-test",
    feature = "initrd-wx-init-test"
))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpawnPathError {
    Filesystem(FsError),
    NotRegular,
    InvalidName,
    Process(crate::process::SpawnElfError),
}
