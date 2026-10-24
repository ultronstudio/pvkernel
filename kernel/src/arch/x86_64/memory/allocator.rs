//! Fixed-range bitmap kernel heap allocator.
//!
//! The heap is divided into 16-byte units. A static bitmap tracks occupied
//! units and allocation starts, so allocator metadata never lives in the
//! heap and allocator bookkeeping does not dereference raw pointers. First-fit
//! search naturally reuses and coalesces adjacent free units.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::null_mut;
use spin::Mutex;

use crate::kprintln;

const ALLOCATION_GRANULARITY: usize = 16;
const MAX_HEAP_SIZE: usize = super::heap::HEAP_SIZE;
const MAX_HEAP_UNITS: usize = MAX_HEAP_SIZE / ALLOCATION_GRANULARITY;
const BITMAP_WORD_BITS: usize = usize::BITS as usize;
const BITMAP_WORDS: usize = MAX_HEAP_UNITS.div_ceil(BITMAP_WORD_BITS);

struct HeapAllocator {
    start: usize,
    units: usize,
    occupied: [usize; BITMAP_WORDS],
    allocation_starts: [usize; BITMAP_WORDS],
    initialized: bool,
}

impl HeapAllocator {
    const fn new() -> Self {
        Self {
            start: 0,
            units: 0,
            occupied: [0; BITMAP_WORDS],
            allocation_starts: [0; BITMAP_WORDS],
            initialized: false,
        }
    }

    fn bit_is_set(bitmap: &[usize; BITMAP_WORDS], unit: usize) -> bool {
        (bitmap[unit / BITMAP_WORD_BITS] & (1 << (unit % BITMAP_WORD_BITS))) != 0
    }

    fn set_bit(bitmap: &mut [usize; BITMAP_WORDS], unit: usize) {
        bitmap[unit / BITMAP_WORD_BITS] |= 1 << (unit % BITMAP_WORD_BITS);
    }

    fn clear_bit(bitmap: &mut [usize; BITMAP_WORDS], unit: usize) {
        bitmap[unit / BITMAP_WORD_BITS] &= !(1 << (unit % BITMAP_WORD_BITS));
    }

    fn allocation_units(layout: Layout) -> Option<usize> {
        layout
            .size()
            .max(1)
            .checked_add(ALLOCATION_GRANULARITY - 1)
            .map(|size| size / ALLOCATION_GRANULARITY)
    }

    /// Initializes the metadata for one exclusively owned heap range.
    ///
    /// # Safety
    /// The range must be writable, mapped, reserved for this allocator for
    /// the kernel lifetime, and not accessible to any other allocator.
    unsafe fn init(&mut self, start: usize, size: usize) -> Result<(), &'static str> {
        if self.initialized {
            return Err("allocator initialized twice");
        }
        let end = start.checked_add(size).ok_or("heap range overflow")?;
        if !start.is_multiple_of(ALLOCATION_GRANULARITY)
            || !size.is_multiple_of(ALLOCATION_GRANULARITY)
            || size > MAX_HEAP_SIZE
            || size == 0
            || end <= start
        {
            return Err("invalid heap range alignment or size");
        }

        self.start = start;
        self.units = size / ALLOCATION_GRANULARITY;
        self.occupied.fill(0);
        self.allocation_starts.fill(0);
        self.initialized = true;
        Ok(())
    }

    fn allocate(&mut self, layout: Layout) -> *mut u8 {
        if !self.initialized {
            return null_mut();
        }
        let Some(requested_units) = Self::allocation_units(layout) else {
            return null_mut();
        };
        if requested_units > self.units {
            return null_mut();
        }

        let mut candidate = 0;
        while candidate < self.units {
            let Some(candidate_offset) = candidate.checked_mul(ALLOCATION_GRANULARITY) else {
                return null_mut();
            };
            let Some(candidate_address) = self.start.checked_add(candidate_offset) else {
                return null_mut();
            };
            let Some(aligned_address) = align_up(candidate_address, layout.align()) else {
                return null_mut();
            };
            let Some(aligned_offset) = aligned_address.checked_sub(self.start) else {
                return null_mut();
            };
            let first_unit = aligned_offset / ALLOCATION_GRANULARITY;
            let Some(end_unit) = first_unit.checked_add(requested_units) else {
                return null_mut();
            };
            if end_unit > self.units {
                return null_mut();
            }

            let mut occupied_unit = None;
            for unit in first_unit..end_unit {
                if Self::bit_is_set(&self.occupied, unit) {
                    occupied_unit = Some(unit);
                    break;
                }
            }
            if let Some(unit) = occupied_unit {
                candidate = unit + 1;
                continue;
            }

            for unit in first_unit..end_unit {
                Self::set_bit(&mut self.occupied, unit);
            }
            Self::set_bit(&mut self.allocation_starts, first_unit);
            return aligned_address as *mut u8;
        }
        null_mut()
    }

    /// Releases exactly one live allocation.
    ///
    /// The occupancy and start bitmaps validate the pointer and exact run
    /// length before clearing anything, rejecting interior pointers, bad
    /// layouts, and double frees without mutating allocator state.
    fn deallocate(&mut self, ptr: *mut u8, layout: Layout) -> bool {
        if !self.initialized || ptr.is_null() {
            return false;
        }
        let Some(expected_units) = Self::allocation_units(layout) else {
            return false;
        };
        let address = ptr as usize;
        let Some(offset) = address.checked_sub(self.start) else {
            return false;
        };
        if !offset.is_multiple_of(ALLOCATION_GRANULARITY) || !address.is_multiple_of(layout.align())
        {
            return false;
        }
        let first_unit = offset / ALLOCATION_GRANULARITY;
        if first_unit >= self.units
            || !Self::bit_is_set(&self.occupied, first_unit)
            || !Self::bit_is_set(&self.allocation_starts, first_unit)
        {
            return false;
        }

        let mut actual_units = 0;
        for unit in first_unit..self.units {
            if !Self::bit_is_set(&self.occupied, unit)
                || (unit != first_unit && Self::bit_is_set(&self.allocation_starts, unit))
            {
                break;
            }
            actual_units += 1;
        }
        if actual_units != expected_units {
            return false;
        }

        for unit in first_unit..first_unit + actual_units {
            Self::clear_bit(&mut self.occupied, unit);
        }
        Self::clear_bit(&mut self.allocation_starts, first_unit);
        true
    }
}

fn align_up(value: usize, alignment: usize) -> Option<usize> {
    value
        .checked_add(alignment.checked_sub(1)?)
        .map(|aligned| aligned & !(alignment - 1))
}

/// Global allocator used by `alloc`. Before heap initialization, allocation
/// returns null, making accidental early allocation fail deterministically.
pub struct KernelGlobalAllocator(Mutex<HeapAllocator>);

impl KernelGlobalAllocator {
    pub const fn new() -> Self {
        Self(Mutex::new(HeapAllocator::new()))
    }

    /// Initializes the global heap exactly once.
    ///
    /// # Safety
    /// The range must already be mapped writable, supervisor-only, NX, and
    /// exclusively reserved for the allocator for the kernel lifetime.
    pub unsafe fn init(&self, start: usize, size: usize) -> Result<(), &'static str> {
        x86_64::instructions::interrupts::without_interrupts(|| {
            // SAFETY: forwarded from this method's contract; the mutex
            // serializes the one-time mutation.
            unsafe { self.0.lock().init(start, size) }
        })
    }
}

#[global_allocator]
pub static ALLOCATOR: KernelGlobalAllocator = KernelGlobalAllocator::new();

unsafe impl GlobalAlloc for KernelGlobalAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        x86_64::instructions::interrupts::without_interrupts(|| self.0.lock().allocate(layout))
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let valid = x86_64::instructions::interrupts::without_interrupts(|| {
            // SAFETY: GlobalAlloc's caller must pass a live allocation from
            // this allocator with its original layout.
            self.0.lock().deallocate(ptr, layout)
        });
        if !valid {
            kprintln!("[heap] invalid free, layout, or overlapping heap region");
            crate::halt::halt_forever();
        }
    }
}
