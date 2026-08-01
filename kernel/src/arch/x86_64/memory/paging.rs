//! Focused 4 KiB mapping operations and the page-permission model.
//!
//! Every function here operates on a caller-supplied [`OffsetPageTable`] and
//! [`BootFrameAllocator`] rather than reaching into global state directly;
//! [`super::address_space`] owns the single, lock-protected instance of
//! both that pvKernel actually uses.

use x86_64::{
    PhysAddr, VirtAddr,
    structures::paging::{
        FrameAllocator, Mapper, OffsetPageTable, Page, PageTableFlags, PhysFrame, Size4KiB,
        Translate,
        mapper::{FlagUpdateError, MapToError, MappedFrame, TranslateResult, UnmapError},
    },
};

/// Requested access permissions for a new or updated 4 KiB mapping.
///
/// Mappings created through this module are supervisor-only unless
/// `user_accessible` is explicitly set, and non-executable unless
/// `executable` is explicitly set. [`Permissions::to_flags`] refuses to
/// produce a writable-and-executable combination: that is pvKernel's W^X
/// policy for every mapping created through this safe API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    pub writable: bool,
    pub executable: bool,
    pub user_accessible: bool,
}

impl Permissions {
    /// Read-write, non-executable, supervisor-only. The default shape for
    /// ordinary kernel data pages.
    pub const KERNEL_DATA: Self = Self {
        writable: true,
        executable: false,
        user_accessible: false,
    };

    /// Read-only, non-executable, supervisor-only.
    pub const KERNEL_READ_ONLY: Self = Self {
        writable: false,
        executable: false,
        user_accessible: false,
    };

    /// Read-only, executable, supervisor-only. New executable mappings
    /// default to read-only, per pvKernel's W^X policy.
    ///
    /// pvKernel 0.0.5 never creates an executable mapping itself (there is
    /// no ELF loader yet); this constant exists so the permission model
    /// documented in `docs/architecture/virtual-memory.md` is already
    /// complete for the next milestone that needs it.
    #[allow(dead_code)]
    pub const KERNEL_EXECUTABLE: Self = Self {
        writable: false,
        executable: true,
        user_accessible: false,
    };

    fn to_flags(self) -> Result<PageTableFlags, MemoryError> {
        if self.writable && self.executable {
            return Err(MemoryError::InvalidPermissions);
        }

        let mut flags = PageTableFlags::PRESENT;
        if self.writable {
            flags |= PageTableFlags::WRITABLE;
        }
        if !self.executable {
            flags |= PageTableFlags::NO_EXECUTE;
        }
        if self.user_accessible {
            flags |= PageTableFlags::USER_ACCESSIBLE;
        }
        Ok(flags)
    }
}

/// Errors produced by the memory-mapping API in this module.
///
/// Reuses information from the underlying `x86_64` crate errors where
/// possible, wrapped into a single project-specific type so callers do not
/// need to match on multiple upstream error enums.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryError {
    /// The virtual address is not in canonical form.
    NonCanonicalAddress,
    /// The virtual address is not aligned to a 4 KiB page boundary.
    Unaligned,
    /// The page is already mapped to a physical frame.
    AlreadyMapped,
    /// The page is not mapped to a physical frame.
    NotMapped,
    /// The frame allocator has no more usable frames.
    FrameAllocationFailed,
    /// A parent page-table entry is part of an already-mapped huge page.
    ParentEntryHugePage,
    /// The requested permission combination is not allowed (for example,
    /// writable and executable at the same time).
    InvalidPermissions,
    /// A page-table entry pointed at an invalid physical address.
    InvalidFrameAddress,
}

impl core::fmt::Display for MemoryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let message = match self {
            MemoryError::NonCanonicalAddress => "virtual address is not canonical",
            MemoryError::Unaligned => "address is not 4 KiB aligned",
            MemoryError::AlreadyMapped => "page is already mapped",
            MemoryError::NotMapped => "page is not mapped",
            MemoryError::FrameAllocationFailed => "no usable physical frames remain",
            MemoryError::ParentEntryHugePage => "parent page-table entry is a huge page",
            MemoryError::InvalidPermissions => "writable and executable requested together",
            MemoryError::InvalidFrameAddress => "page-table entry points at an invalid frame",
        };
        f.write_str(message)
    }
}

/// The result of translating a virtual address, as returned by
/// [`translate`]. Distinguishes an ordinary 4 KiB mapping from a huge-page
/// mapping so callers cannot mistake one for the other.
#[derive(Debug, Clone, Copy)]
pub enum Translation {
    /// The address is mapped to an ordinary 4 KiB frame.
    Mapped {
        phys_addr: PhysAddr,
        flags: PageTableFlags,
    },
    /// The address is mapped, but through a 2 MiB or 1 GiB huge page.
    ///
    /// pvKernel 0.0.5 never creates a huge-page mapping itself, and its
    /// boot-time tests never translate an address the bootloader mapped as
    /// one; `phys_addr` is populated for completeness and future callers
    /// (e.g. a debugging command) even though nothing in this crate reads
    /// it yet.
    #[allow(dead_code)]
    HugePage { phys_addr: PhysAddr },
    /// The address has no mapping.
    NotMapped,
    /// The page-table entry for this address points at an invalid physical
    /// address.
    Unsupported,
}

/// Converts a raw address into a canonical [`VirtAddr`], without panicking
/// on an ordinary non-canonical address.
pub fn canonical_virt_addr(addr: u64) -> Result<VirtAddr, MemoryError> {
    VirtAddr::try_new(addr).map_err(|_| MemoryError::NonCanonicalAddress)
}

/// Converts a canonical virtual address into a page-aligned [`Page`].
pub fn page_at(addr: VirtAddr) -> Result<Page<Size4KiB>, MemoryError> {
    Page::from_start_address(addr).map_err(|_| MemoryError::Unaligned)
}

/// Maps `page` to `frame` with the given `permissions`.
///
/// Allocates any missing intermediate page tables from `frame_allocator`.
/// Flushes the mapped page's TLB entry before returning on success.
pub fn map_page(
    mapper: &mut OffsetPageTable<'static>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    page: Page<Size4KiB>,
    frame: PhysFrame<Size4KiB>,
    permissions: Permissions,
) -> Result<(), MemoryError> {
    let flags = permissions.to_flags()?;

    // SAFETY: `map_to` is unsafe because creating an arbitrary mapping can
    // alias `&mut` references or corrupt in-use values. `frame` is expected
    // to be a frame the caller has not mapped anywhere else (in practice,
    // one just returned by `frame_allocator`); `map_to` itself refuses to
    // remap a page that is already mapped (`PageAlreadyMapped`), so the only
    // remaining caller obligation is not passing in a frame that is already
    // in use elsewhere, which is documented on the callers of this
    // crate-internal function.
    let result = unsafe { mapper.map_to(page, frame, flags, frame_allocator) };
    match result {
        Ok(flush) => {
            flush.flush();
            Ok(())
        }
        Err(MapToError::FrameAllocationFailed) => Err(MemoryError::FrameAllocationFailed),
        Err(MapToError::ParentEntryHugePage) => Err(MemoryError::ParentEntryHugePage),
        Err(MapToError::PageAlreadyMapped(_)) => Err(MemoryError::AlreadyMapped),
    }
}

/// Unmaps `page`, flushes its TLB entry, and returns the physical frame it
/// was mapped to.
///
/// Note that the returned frame is not returned to any allocator: this
/// milestone's [`super::frame_allocator::BootFrameAllocator`] cannot accept
/// it back, so callers must not treat the frame as reusable.
pub fn unmap_page(
    mapper: &mut OffsetPageTable<'static>,
    page: Page<Size4KiB>,
) -> Result<PhysFrame<Size4KiB>, MemoryError> {
    match mapper.unmap(page) {
        Ok((frame, flush)) => {
            flush.flush();
            Ok(frame)
        }
        Err(UnmapError::PageNotMapped) => Err(MemoryError::NotMapped),
        Err(UnmapError::ParentEntryHugePage) => Err(MemoryError::ParentEntryHugePage),
        Err(UnmapError::InvalidFrameAddress(_)) => Err(MemoryError::InvalidFrameAddress),
    }
}

/// Changes the permissions of an existing mapping and flushes its TLB
/// entry.
pub fn update_permissions(
    mapper: &mut OffsetPageTable<'static>,
    page: Page<Size4KiB>,
    permissions: Permissions,
) -> Result<(), MemoryError> {
    let flags = permissions.to_flags()?;

    // SAFETY: this only changes the flags of a mapping already owned by the
    // caller of this crate-internal function (in practice, a temporary test
    // page); the `GLOBAL` flag is never set here, so there is no
    // cross-address-space consistency requirement to uphold.
    let result = unsafe { mapper.update_flags(page, flags) };
    match result {
        Ok(flush) => {
            flush.flush();
            Ok(())
        }
        Err(FlagUpdateError::PageNotMapped) => Err(MemoryError::NotMapped),
        Err(FlagUpdateError::ParentEntryHugePage) => Err(MemoryError::ParentEntryHugePage),
    }
}

/// Translates `addr` to a physical address, distinguishing an unmapped
/// address from a huge-page mapping. Never panics on an ordinary unmapped
/// address.
pub fn translate(mapper: &OffsetPageTable<'static>, addr: VirtAddr) -> Translation {
    match mapper.translate(addr) {
        TranslateResult::Mapped {
            frame: MappedFrame::Size4KiB(frame),
            offset,
            flags,
        } => Translation::Mapped {
            phys_addr: frame.start_address() + offset,
            flags,
        },
        TranslateResult::Mapped { frame, offset, .. } => Translation::HugePage {
            phys_addr: frame.start_address() + offset,
        },
        TranslateResult::NotMapped => Translation::NotMapped,
        TranslateResult::InvalidFrameAddress(_) => Translation::Unsupported,
    }
}
