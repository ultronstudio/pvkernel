//! A monotonic boot-time physical frame allocator.
//!
//! This allocator is built directly from the bootloader-provided physical
//! memory map and only ever hands out 4 KiB frames from regions the
//! bootloader explicitly marked [`MemoryRegionKind::Usable`]. It never
//! infers usability from an address range, never allocates the same frame
//! twice, and never allocates a frame outside a usable region.

use bootloader_api::info::{MemoryRegion, MemoryRegionKind};
use x86_64::{
    PhysAddr,
    structures::paging::{FrameAllocator, PageSize, PhysFrame, Size4KiB},
};

const FRAME_SIZE: u64 = Size4KiB::SIZE;

const fn align_up(addr: u64, align: u64) -> u64 {
    (addr + align - 1) & !(align - 1)
}

const fn align_down(addr: u64, align: u64) -> u64 {
    addr & !(align - 1)
}

/// Hands out previously unused 4 KiB physical frames from the usable
/// regions of the bootloader's memory map, in ascending address order,
/// without ever repeating a frame.
///
/// # Limitations
///
/// This is a monotonic boot-time allocator, not a general-purpose physical
/// memory manager: it has no [`x86_64::structures::paging::FrameDeallocator`]
/// implementation, and none is claimed. Once a frame is handed out it is
/// considered permanently in use for the life of the kernel. This is
/// sufficient for the intermediate page tables and test mappings created in
/// pvKernel 0.0.5, but frame reclamation is out of scope for this milestone.
pub struct BootFrameAllocator {
    regions: &'static [MemoryRegion],
    next: usize,
    allocated: usize,
}

impl BootFrameAllocator {
    /// Builds an allocator over the given memory map.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that `regions` is the actual, unmodified
    /// physical memory map supplied by the bootloader (so that regions
    /// marked [`MemoryRegionKind::Usable`] are genuinely free of the
    /// kernel image, bootloader structures, firmware, ACPI data, and
    /// runtime services), and that no other allocator concurrently hands
    /// out frames from the same regions.
    pub unsafe fn new(regions: &'static [MemoryRegion]) -> Self {
        Self {
            regions,
            next: 0,
            allocated: 0,
        }
    }

    /// Iterates every allocatable 4 KiB frame in ascending address order.
    ///
    /// Region starts are aligned up and region ends are aligned down to the
    /// frame size, so a partially-aligned usable region never yields a
    /// frame that spills outside it.
    fn usable_frames(&self) -> impl Iterator<Item = PhysFrame<Size4KiB>> + '_ {
        self.regions
            .iter()
            .filter(|region| region.kind == MemoryRegionKind::Usable)
            .flat_map(|region| {
                let start = align_up(region.start, FRAME_SIZE);
                let end = align_down(region.end, FRAME_SIZE);
                let frame_count = end.saturating_sub(start) / FRAME_SIZE;
                (0..frame_count).map(move |i| {
                    PhysFrame::containing_address(PhysAddr::new(start + i * FRAME_SIZE))
                })
            })
    }

    /// Total number of 4 KiB frames this allocator could ever hand out,
    /// including ones already allocated. Recomputed from the memory map on
    /// every call; intended for occasional boot diagnostics, not a hot path.
    pub fn usable_frame_count(&self) -> usize {
        self.usable_frames().count()
    }

    /// Number of frames handed out so far.
    pub fn allocated_frame_count(&self) -> usize {
        self.allocated
    }
}

// SAFETY: `allocate_frame` only ever returns a frame at the current `next`
// index and then advances `next`, so no frame is ever returned twice by this
// allocator; combined with the safety invariant on `new` (the region list is
// the real usable memory map), this satisfies `FrameAllocator`'s contract
// that returned frames are unique and unused.
unsafe impl FrameAllocator<Size4KiB> for BootFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        let frame = self.usable_frames().nth(self.next);
        if frame.is_some() {
            self.next += 1;
            self.allocated += 1;
        }
        frame
    }
}
