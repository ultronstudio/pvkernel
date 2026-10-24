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
    cursor: FrameCursor,
    allocated: usize,
}

#[derive(Default)]
struct FrameCursor {
    region_index: usize,
    next_frame_address: u64,
}

impl FrameCursor {
    fn next_address(&mut self, regions: &[MemoryRegion]) -> Option<u64> {
        while self.region_index < regions.len() {
            let region = &regions[self.region_index];
            if region.kind != MemoryRegionKind::Usable {
                self.region_index += 1;
                self.next_frame_address = 0;
                continue;
            }
            let Some(rounded_start) = region.start.checked_add(FRAME_SIZE - 1) else {
                self.region_index += 1;
                self.next_frame_address = 0;
                continue;
            };
            let start = align_down(rounded_start, FRAME_SIZE);
            let end = align_down(region.end, FRAME_SIZE);
            let address = self.next_frame_address.max(start);
            if address < end {
                let Some(next_address) = address.checked_add(FRAME_SIZE) else {
                    self.region_index += 1;
                    self.next_frame_address = 0;
                    continue;
                };
                self.next_frame_address = next_address;
                return Some(address);
            }
            self.region_index += 1;
            self.next_frame_address = 0;
        }
        None
    }
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
            cursor: FrameCursor::default(),
            allocated: 0,
        }
    }

    /// Counts allocatable frames. This diagnostic scan is O(number of
    /// regions) and is never used by the allocation hot path.
    pub fn usable_frame_count(&self) -> usize {
        self.regions
            .iter()
            .filter(|region| region.kind == MemoryRegionKind::Usable)
            .map(|region| {
                let Some(start) = region.start.checked_add(FRAME_SIZE - 1) else {
                    return 0;
                };
                let start = align_down(start, FRAME_SIZE);
                let end = align_down(region.end, FRAME_SIZE);
                end.saturating_sub(start) / FRAME_SIZE
            })
            .fold(0_u64, u64::saturating_add)
            .try_into()
            .unwrap_or(usize::MAX)
    }

    /// Number of frames handed out so far.
    pub fn allocated_frame_count(&self) -> usize {
        self.allocated
    }
}

// SAFETY: `allocate_frame` advances a persistent region/address cursor, so no
// frame is ever returned twice by this
// allocator; combined with the safety invariant on `new` (the region list is
// the real usable memory map), this satisfies `FrameAllocator`'s contract
// that returned frames are unique and unused.
unsafe impl FrameAllocator<Size4KiB> for BootFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        let address = self.cursor.next_address(self.regions)?;
        self.allocated = self.allocated.saturating_add(1);
        Some(PhysFrame::containing_address(PhysAddr::new(address)))
    }
}

#[cfg(feature = "qemu-test")]
pub fn run_cursor_regression_test() {
    static REGIONS: [MemoryRegion; 5] = [
        MemoryRegion {
            start: 0x1003,
            end: 0x3001,
            kind: MemoryRegionKind::Usable,
        },
        MemoryRegion {
            start: 0x4000,
            end: 0x8000,
            kind: MemoryRegionKind::Bootloader,
        },
        MemoryRegion {
            start: 0x9000,
            end: 0xB000,
            kind: MemoryRegionKind::Usable,
        },
        MemoryRegion {
            start: 0xC001,
            end: 0xCFFF,
            kind: MemoryRegionKind::Usable,
        },
        MemoryRegion {
            start: u64::MAX - 3,
            end: u64::MAX,
            kind: MemoryRegionKind::Usable,
        },
    ];
    let mut cursor = FrameCursor::default();
    let frames: [u64; 3] = core::array::from_fn(|_| cursor.next_address(&REGIONS).unwrap());
    assert_eq!(frames, [0x2000, 0x9000, 0xA000]);
    assert_eq!(cursor.next_address(&REGIONS), None);
    crate::kprintln!("[memory-test] stateful frame cursor regression passed");
}
