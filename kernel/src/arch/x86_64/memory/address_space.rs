//! Ownership of the active x86_64 page table and the boot frame allocator.
//!
//! There is exactly one [`KernelAddressSpace`] for the whole kernel in this
//! milestone (pvKernel 0.0.5 does not create per-process address spaces).
//! It is built once during [`super::init`] and afterward is only reachable
//! through [`with_kernel_address_space`], which serializes access behind a
//! [`spin::Mutex`] and disables maskable interrupts for the duration of the
//! call. This keeps a single owner of the mapper at all times: no code path
//! can obtain two simultaneous mutable references to the page tables.

use spin::{Mutex, Once};
use x86_64::{
    PhysAddr, VirtAddr,
    registers::control::Cr3,
    structures::paging::{FrameAllocator, OffsetPageTable, Page, PageTable, PhysFrame, Size4KiB},
};

use super::frame_allocator::BootFrameAllocator;
use super::paging::{self, MemoryError, Permissions, Translation};

static KERNEL_ADDRESS_SPACE: Once<Mutex<KernelAddressSpace>> = Once::new();

/// The kernel's active page-table mapper plus the boot-time physical frame
/// allocator, held behind one lock so the two can never fall out of sync
/// with each other.
pub struct KernelAddressSpace {
    mapper: OffsetPageTable<'static>,
    frame_allocator: BootFrameAllocator,
}

impl KernelAddressSpace {
    /// Allocates a fresh physical frame and maps `page` to it with
    /// `permissions`. Returns the frame that was mapped.
    pub fn map_new_page(
        &mut self,
        page: Page<Size4KiB>,
        permissions: Permissions,
    ) -> Result<PhysFrame<Size4KiB>, MemoryError> {
        let frame = self
            .frame_allocator
            .allocate_frame()
            .ok_or(MemoryError::FrameAllocationFailed)?;
        paging::map_page(
            &mut self.mapper,
            &mut self.frame_allocator,
            page,
            frame,
            permissions,
        )?;
        Ok(frame)
    }

    /// Unmaps `page` and flushes its TLB entry. See
    /// [`paging::unmap_page`] for why the returned frame cannot be reused.
    pub fn unmap_page(&mut self, page: Page<Size4KiB>) -> Result<PhysFrame<Size4KiB>, MemoryError> {
        paging::unmap_page(&mut self.mapper, page)
    }

    /// Changes the permissions of an existing mapping and flushes its TLB
    /// entry.
    pub fn update_page_permissions(
        &mut self,
        page: Page<Size4KiB>,
        permissions: Permissions,
    ) -> Result<(), MemoryError> {
        paging::update_permissions(&mut self.mapper, page, permissions)
    }

    /// Translates a virtual address to its current mapping state.
    pub fn translate(&self, addr: VirtAddr) -> Translation {
        paging::translate(&self.mapper, addr)
    }

    /// Total number of 4 KiB frames the boot frame allocator could ever
    /// hand out. See [`BootFrameAllocator`] for why this is only a boot-time
    /// figure, not a live free-memory count.
    pub fn usable_frame_count(&self) -> usize {
        self.frame_allocator.usable_frame_count()
    }

    /// Number of frames the boot frame allocator has handed out so far.
    pub fn allocated_frame_count(&self) -> usize {
        self.frame_allocator.allocated_frame_count()
    }
}

/// Returns a mutable reference to the currently active level-4 page table.
///
/// Callers must not reach this function before
/// `super::validate_paging_prerequisites` has confirmed CR4.LA57 is clear:
/// `Cr3::read()` always returns a frame address regardless of paging mode,
/// but interpreting it as a four-level `PageTable` (as this function and
/// `OffsetPageTable` both do) is only correct in that mode. `super::init`
/// enforces the ordering (validate, then call this function), so by the
/// time this runs, four-level paging is a confirmed precondition, not an
/// assumption.
///
/// # Safety
///
/// The caller must guarantee that:
///
/// - four-level paging is active (CR4.LA57 clear), per the above;
/// - `phys_mem_offset` is the actual virtual address at which the
///   bootloader mapped the whole of physical memory (i.e. the bootloader
///   was configured with `mappings.physical_memory = Some(Mapping::Dynamic)`
///   and `phys_mem_offset` came from `BootInfo::physical_memory_offset`);
/// - the active CR3 frame is therefore reachable at
///   `phys_mem_offset + CR3_physical_address`;
/// - this function is called at most once for the life of the kernel (or,
///   if called again, that no other `&mut PageTable` obtained from a
///   previous call is still alive), so no two mutable references to the
///   same page table ever coexist;
/// - the returned reference does not outlive the physical-memory mapping it
///   depends on (which, once established at boot, lasts for the kernel's
///   entire lifetime).
unsafe fn active_level_4_table(phys_mem_offset: VirtAddr) -> &'static mut PageTable {
    let (level_4_frame, _) = Cr3::read();
    let phys: PhysAddr = level_4_frame.start_address();
    let virt = phys_mem_offset + phys.as_u64();
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();

    // SAFETY: forwarded from this function's own safety contract above; the
    // pointer was computed from the live CR3 frame and the caller-verified
    // physical-memory offset, so it points at the actual active level-4
    // table, and the single-call invariant means this is the only mutable
    // reference to it.
    unsafe { &mut *page_table_ptr }
}

/// Builds the single kernel address space. Must be called at most once; see
/// [`super::init`], which enforces this through [`Once::call_once`].
pub(super) fn init(phys_mem_offset: VirtAddr, frame_allocator: BootFrameAllocator) {
    KERNEL_ADDRESS_SPACE.call_once(|| {
        // SAFETY: `phys_mem_offset` is forwarded unchanged from
        // `BootInfo::physical_memory_offset` by `super::init`, which itself
        // only runs this closure once via `Once::call_once`, so no
        // competing mutable reference to the level-4 table is created
        // elsewhere.
        let level_4_table = unsafe { active_level_4_table(phys_mem_offset) };

        // SAFETY: `level_4_table` was just obtained above and is not
        // referenced anywhere else; `phys_mem_offset` is the same value
        // used to locate it, which is exactly what `OffsetPageTable::new`
        // requires.
        let mapper = unsafe { OffsetPageTable::new(level_4_table, phys_mem_offset) };

        Mutex::new(KernelAddressSpace {
            mapper,
            frame_allocator,
        })
    });
}

/// Runs `f` with exclusive access to the kernel address space.
///
/// Disables maskable hardware interrupts for the duration of the call. No
/// current interrupt handler touches page-table state, but this keeps that
/// invariant explicit and enforced rather than implicit, and prevents a
/// future handler from ever observing a page table mid-update.
///
/// # Panics
///
/// Panics if [`init`] has not run yet; this indicates a boot-ordering bug
/// (memory subsystem used before `memory::init` ran), not an ordinary
/// runtime condition.
pub fn with_kernel_address_space<R>(f: impl FnOnce(&mut KernelAddressSpace) -> R) -> R {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let lock = KERNEL_ADDRESS_SPACE
            .get()
            .expect("memory subsystem used before memory::init ran");
        let mut guard = lock.lock();
        f(&mut guard)
    })
}
