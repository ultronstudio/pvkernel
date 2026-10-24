//! Fixed-range, first-fit kernel heap allocator.
//!
//! Free nodes live inside free heap regions. Allocations carry no in-band
//! metadata: `dealloc` receives the original `Layout`, whose size is rounded
//! to the node alignment in exactly the same way as `alloc`.

use core::alloc::{GlobalAlloc, Layout};
use core::mem::{align_of, size_of};
use core::ptr::{self, null_mut};
use spin::Mutex;

use crate::kprintln;

const NODE_ALIGN: usize = align_of::<ListNode>();
const NODE_SIZE: usize = size_of::<ListNode>();

#[derive(Clone, Copy)]
struct ListNode {
    size: usize,
    next: *mut ListNode,
}

struct HeapAllocator {
    start: usize,
    end: usize,
    head: *mut ListNode,
    initialized: bool,
}

// SAFETY: the allocator is only accessed through the enclosing spin mutex.
unsafe impl Send for HeapAllocator {}

impl HeapAllocator {
    const fn new() -> Self {
        Self {
            start: 0,
            end: 0,
            head: null_mut(),
            initialized: false,
        }
    }

    /// Checks a free-list node's address before any pointer dereference.
    fn valid_node_address(&self, node: *const ListNode) -> bool {
        if !self.initialized || node.is_null() {
            return false;
        }
        let address = node as usize;
        address >= self.start
            && address.is_multiple_of(NODE_ALIGN)
            && address
                .checked_add(NODE_SIZE)
                .is_some_and(|node_end| node_end <= self.end)
    }

    /// Initializes one free region without allocating metadata.
    ///
    /// # Safety
    /// `start..start + size` must be a writable, mapped heap range that no
    /// other owner can use, with `start` and `size` aligned to `NODE_ALIGN`.
    unsafe fn init(&mut self, start: usize, size: usize) -> Result<(), &'static str> {
        if self.initialized {
            return Err("allocator initialized twice");
        }
        let end = start.checked_add(size).ok_or("heap range overflow")?;
        if !start.is_multiple_of(NODE_ALIGN) || size < NODE_SIZE || !size.is_multiple_of(NODE_ALIGN)
        {
            return Err("invalid heap range alignment or size");
        }
        self.start = start;
        self.end = end;
        // SAFETY: the caller guarantees this mapped range is writable and
        // exclusively owned; `start` is aligned for `ListNode`.
        unsafe {
            ptr::write(
                start as *mut ListNode,
                ListNode {
                    size,
                    next: null_mut(),
                },
            )
        };
        self.head = start as *mut ListNode;
        self.initialized = true;
        Ok(())
    }

    fn allocate(&mut self, layout: Layout) -> *mut u8 {
        if !self.initialized {
            return null_mut();
        }
        let Some(size) = layout.size().max(1).checked_add(NODE_ALIGN - 1) else {
            return null_mut();
        };
        let size = (size & !(NODE_ALIGN - 1)).max(NODE_SIZE);
        let mut previous: *mut ListNode = null_mut();
        let mut current = self.head;

        while !current.is_null() {
            if !self.valid_node_address(current) {
                return null_mut();
            }
            // SAFETY: every list node is created only inside the heap from a
            // free region; `valid_node_address` checks its alignment and
            // complete metadata extent, and the lock protects list links.
            let node = unsafe { &mut *current };
            let block_start = current as usize;
            let Some(aligned_start) = align_up(block_start, layout.align()) else {
                previous = current;
                current = node.next;
                continue;
            };
            let prefix = aligned_start - block_start;
            let Some(allocation_end) = aligned_start.checked_add(size) else {
                previous = current;
                current = node.next;
                continue;
            };
            let Some(block_end) = block_start.checked_add(node.size) else {
                return null_mut();
            };
            if node.size < NODE_SIZE || block_end > self.end {
                return null_mut();
            }
            if aligned_start < block_start
                || allocation_end > block_end
                || (prefix != 0 && prefix < NODE_SIZE)
            {
                previous = current;
                current = node.next;
                continue;
            }
            let suffix = block_end - allocation_end;
            if suffix != 0 && suffix < NODE_SIZE {
                previous = current;
                current = node.next;
                continue;
            }

            let next = node.next;
            if !next.is_null() && !self.valid_node_address(next) {
                return null_mut();
            }
            let replacement = if suffix >= NODE_SIZE {
                let suffix_node = allocation_end as *mut ListNode;
                // SAFETY: the suffix is inside the original free node, has
                // at least NODE_SIZE bytes, and starts NODE_ALIGN-aligned.
                unsafe { ptr::write(suffix_node, ListNode { size: suffix, next }) };
                suffix_node
            } else {
                next
            };

            if prefix >= NODE_SIZE {
                // SAFETY: the existing node remains a valid free prefix.
                unsafe {
                    ptr::write(
                        current,
                        ListNode {
                            size: prefix,
                            next: replacement,
                        },
                    )
                };
            } else if previous.is_null() {
                self.head = replacement;
            } else {
                // SAFETY: `previous` is a valid free-list node, as established
                // by the traversal and rechecked here; the lock prevents
                // concurrent edits.
                if !self.valid_node_address(previous) {
                    return null_mut();
                }
                unsafe { (*previous).next = replacement };
            }
            return aligned_start as *mut u8;
        }
        null_mut()
    }

    /// Reinserts a freed region in address order and coalesces neighbors.
    /// Returns false for a pointer/range outside this heap or an overlap.
    unsafe fn deallocate(&mut self, ptr: *mut u8, layout: Layout) -> bool {
        if !self.initialized || ptr.is_null() {
            return false;
        }
        let Some(rounded_size) = layout.size().max(1).checked_add(NODE_ALIGN - 1) else {
            return false;
        };
        let size = (rounded_size & !(NODE_ALIGN - 1)).max(NODE_SIZE);
        let start = ptr as usize;
        let Some(end) = start.checked_add(size) else {
            return false;
        };
        if start < self.start
            || end > self.end
            || !start.is_multiple_of(NODE_ALIGN)
            || size < NODE_SIZE
        {
            return false;
        }

        let mut previous: *mut ListNode = null_mut();
        let mut next = self.head;
        while !next.is_null() && (next as usize) < start {
            if !self.valid_node_address(next) {
                return false;
            }
            previous = next;
            // SAFETY: `valid_node_address` verifies `next` is aligned and its
            // complete metadata region is inside this mapped heap.
            next = unsafe { (*next).next };
        }
        if (!previous.is_null() && !self.valid_node_address(previous))
            || (!next.is_null() && !self.valid_node_address(next))
        {
            return false;
        }

        let previous_end = if previous.is_null() {
            self.start
        } else {
            // SAFETY: `previous` is a valid free-list node reached above.
            let node = unsafe { &*previous };
            let Some(end) = (previous as usize).checked_add(node.size) else {
                return false;
            };
            end
        };
        if start < previous_end || (!next.is_null() && end > next as usize) {
            return false;
        }

        let mut merged_start = start;
        let mut merged_size = size;
        let mut successor = next;
        if previous_end == start && !previous.is_null() {
            // SAFETY: `previous` is the adjacent free region and metadata is
            // valid under the list invariants.
            merged_start = previous as usize;
            let Some(coalesced_size) = (unsafe { (*previous).size }).checked_add(size) else {
                return false;
            };
            merged_size = coalesced_size;
            successor = unsafe { (*previous).next };
        }
        if !successor.is_null() && merged_start.checked_add(merged_size) == Some(successor as usize)
        {
            // SAFETY: `successor` is the adjacent next free region.
            let successor_size = unsafe { (*successor).size };
            let Some(successor_end) = (successor as usize).checked_add(successor_size) else {
                return false;
            };
            if successor_end > self.end || successor_size < NODE_SIZE {
                return false;
            }
            let Some(coalesced_size) = merged_size.checked_add(successor_size) else {
                return false;
            };
            merged_size = coalesced_size;
            successor = unsafe { (*successor).next };
        }

        let merged = merged_start as *mut ListNode;
        let Some(merged_end) = merged_start.checked_add(merged_size) else {
            return false;
        };
        if merged_start < self.start
            || merged_end > self.end
            || !merged_start.is_multiple_of(NODE_ALIGN)
            || merged_size < NODE_SIZE
        {
            return false;
        }
        // SAFETY: the freed region and any coalesced adjacent free regions
        // form a valid mapped, aligned region large enough for a node, as
        // checked above.
        unsafe {
            ptr::write(
                merged,
                ListNode {
                    size: merged_size,
                    next: successor,
                },
            )
        };
        if previous.is_null() {
            self.head = merged;
        } else if merged != previous {
            // SAFETY: `previous` was reached from the list and revalidated
            // above before any list mutation.
            if !self.valid_node_address(previous) {
                return false;
            }
            unsafe { (*previous).next = merged };
        }
        true
    }
}

fn align_up(value: usize, align: usize) -> Option<usize> {
    value
        .checked_add(align.checked_sub(1)?)
        .map(|v| v & !(align - 1))
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
            unsafe { self.0.lock().deallocate(ptr, layout) }
        });
        if !valid {
            kprintln!("[heap] invalid free or overlapping heap region");
            crate::halt::halt_forever();
        }
    }
}
