//! Small, immutable boot-volume support used to load the first process.

pub mod initrd;
pub mod vfs;

use x86_64::VirtAddr;

/// Bootloader-owned immutable bytes retained for the kernel lifetime.
///
/// The bootloader maps the ramdisk in the upper-half dynamic mapping arena;
/// the memory subsystem audits and retains that mapping before this view is
/// constructed. The physical pages are reserved by the bootloader memory map.
#[derive(Clone, Copy)]
pub struct BootRamdisk {
    bytes: &'static [u8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RamdiskError {
    Missing,
    Empty,
    LengthOverflow,
    NonCanonical,
}

impl BootRamdisk {
    /// Validate bootloader mapping metadata before producing an immutable slice.
    ///
    /// # Safety
    /// The pinned bootloader guarantees that a present ramdisk address names a
    /// bootloader-mapped ramdisk of the supplied length. The caller must ensure
    /// `arch::init` completed its boot-mapping audit before invoking this.
    pub unsafe fn from_mapping(start: Option<u64>, raw_len: u64) -> Result<Self, RamdiskError> {
        let start = start.ok_or(RamdiskError::Missing)?;
        let len = usize::try_from(raw_len).map_err(|_| RamdiskError::LengthOverflow)?;
        if len == 0 {
            return Err(RamdiskError::Empty);
        }
        let end = start
            .checked_add(raw_len)
            .ok_or(RamdiskError::LengthOverflow)?;
        if !VirtAddr::try_new(start).is_ok() || !VirtAddr::try_new(end - 1).is_ok() {
            return Err(RamdiskError::NonCanonical);
        }
        if start < crate::arch::x86_64::memory::BOOTLOADER_DYNAMIC_START
            || end > crate::arch::x86_64::memory::BOOTLOADER_DYNAMIC_END
        {
            return Err(RamdiskError::NonCanonical);
        }
        if len > isize::MAX as usize {
            return Err(RamdiskError::LengthOverflow);
        }
        // SAFETY: upheld by this method's contract and the pinned bootloader
        // mapping contract; bounds/canonicality and Rust slice length are checked.
        let bytes = unsafe { core::slice::from_raw_parts(start as *const u8, len) };
        Ok(Self { bytes })
    }

    pub fn bytes(self) -> &'static [u8] {
        self.bytes
    }
}
