//! Fixed 1 MiB kernel heap mapping and boot-time alloc smoke tests.

use alloc::{boxed::Box, string::String, vec::Vec};
use core::alloc::{GlobalAlloc, Layout};
#[cfg(all(feature = "heap-guard-fault-test", feature = "qemu-test"))]
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use x86_64::{
    VirtAddr,
    structures::paging::{Page, PageSize, PageTableFlags, Size4KiB},
};

use super::{Permissions, Translation, paging};
use crate::{arch::x86_64::memory::with_kernel_address_space, kprintln};

pub const HEAP_START: u64 = 0xFFFF_9000_1000_0000;
pub const HEAP_SIZE: usize = 1024 * 1024;
const PAGE_SIZE: u64 = Size4KiB::SIZE;

const fn heap_end() -> u64 {
    match HEAP_START.checked_add(HEAP_SIZE as u64) {
        Some(end) => end,
        None => panic!("kernel heap virtual range overflows"),
    }
}

pub const HEAP_END: u64 = heap_end();

#[cfg(all(feature = "heap-guard-fault-test", feature = "qemu-test"))]
static EXPECTED_GUARD_FAULT_ARMED: AtomicBool = AtomicBool::new(false);
#[cfg(all(feature = "heap-guard-fault-test", feature = "qemu-test"))]
static EXPECTED_GUARD_FAULT_ADDRESS: AtomicU64 = AtomicU64::new(0);

pub fn init() {
    let start = HEAP_START;
    let size = u64::try_from(HEAP_SIZE).expect("heap size does not fit in u64");
    let end = start
        .checked_add(size)
        .expect("invalid heap virtual range: overflow");
    assert_eq!(end, HEAP_END, "heap end constant is inconsistent");
    assert_eq!(start % PAGE_SIZE, 0, "heap start is not page aligned");
    assert_eq!(size % PAGE_SIZE, 0, "heap size is not page compatible");
    let lower_guard = start
        .checked_sub(PAGE_SIZE)
        .expect("invalid lower heap guard");
    let upper_guard = end;
    let page_count = HEAP_SIZE / PAGE_SIZE as usize;

    kprintln!("[heap] mapping kernel heap");
    kprintln!("[heap] range: {:#x}..{:#x}", start, end);
    kprintln!("[heap] size: {} KiB", HEAP_SIZE / 1024);

    with_kernel_address_space(|space| {
        for address in [lower_guard, upper_guard] {
            let virt = paging::canonical_virt_addr(address).expect("heap range is non-canonical");
            assert!(
                matches!(space.translate(virt), Translation::NotMapped),
                "heap or guard address already mapped: {address:#x}"
            );
        }

        for index in 0..page_count {
            let offset = u64::try_from(index)
                .ok()
                .and_then(|index| index.checked_mul(PAGE_SIZE))
                .expect("heap page offset overflows");
            let address = start
                .checked_add(offset)
                .expect("heap page address overflows");
            let virt = paging::canonical_virt_addr(address).expect("heap range is non-canonical");
            assert!(
                matches!(space.translate(virt), Translation::NotMapped),
                "heap page already mapped: {address:#x}"
            );
            let page = Page::<Size4KiB>::from_start_address(VirtAddr::new(address))
                .expect("heap page address is unaligned");
            space
                .map_new_page(page, Permissions::KERNEL_DATA)
                .unwrap_or_else(|error| panic!("failed to map heap page {address:#x}: {error}"));
        }

        for address in [start, start + size / 2, end - PAGE_SIZE] {
            match space.translate(VirtAddr::new(address)) {
                Translation::Mapped { flags, .. } => {
                    assert!(flags.contains(PageTableFlags::PRESENT));
                    assert!(flags.contains(PageTableFlags::WRITABLE));
                    assert!(flags.contains(PageTableFlags::NO_EXECUTE));
                    assert!(!flags.contains(PageTableFlags::USER_ACCESSIBLE));
                }
                other => panic!("heap page {address:#x} is not mapped: {other:?}"),
            }
        }
        for address in [lower_guard, upper_guard] {
            assert!(
                matches!(
                    space.translate(VirtAddr::new(address)),
                    Translation::NotMapped
                ),
                "heap guard page mapped at {address:#x}"
            );
        }
    });

    kprintln!("[heap] pages mapped: {page_count}");
    kprintln!("[heap] supervisor writable NX mappings verified");

    // SAFETY: `init` verified every page is mapped with writable,
    // supervisor-only, NX permissions; the guard pages are outside this
    // range; the range is reserved exclusively for this allocator.
    unsafe {
        super::allocator::ALLOCATOR
            .init(start as usize, HEAP_SIZE)
            .unwrap_or_else(|reason| {
                panic!("kernel heap allocator initialization failed: {reason}")
            });
    }
    kprintln!("[heap] global allocator initialized");
    kprintln!("[heap] initialized");
}

pub fn run_smoke_tests() {
    let boxed = Box::new(0x000A_110C_u64);
    assert_eq!(*boxed, 0x000A_110C);
    assert!(in_heap((&*boxed) as *const u64 as usize));
    kprintln!("[heap-test] Box allocation passed");
    drop(boxed);

    let mut values = Vec::new();
    for value in 0..2048_u32 {
        values.push(value);
    }
    assert_eq!(values.len(), 2048);
    for (index, value) in values.iter().enumerate() {
        assert_eq!(*value, index as u32);
    }
    kprintln!("[heap-test] Vec growth passed");
    drop(values);

    let mut string = String::from("pvKernel ");
    string.push_str("heap alloc works");
    assert_eq!(string, "pvKernel heap alloc works");
    kprintln!("[heap-test] String allocation passed");
    drop(string);

    for alignment in [1, 2, 4, 8, 16, 32, 64, 256, 4096] {
        let layout = Layout::from_size_align(37, alignment).expect("valid test layout");
        // SAFETY: the global allocator has been initialized and the layout
        // is valid; the returned pointer is checked before use and freed with
        // the identical layout.
        let allocation = unsafe { super::allocator::ALLOCATOR.alloc(layout) };
        assert!(
            !allocation.is_null(),
            "alignment allocation failed: {alignment}"
        );
        assert_eq!(
            allocation as usize % alignment,
            0,
            "alignment mismatch: {alignment}"
        );
        assert!(in_heap(allocation as usize));
        // SAFETY: this pointer was returned above for the same layout.
        unsafe { super::allocator::ALLOCATOR.dealloc(allocation, layout) };
    }
    kprintln!("[heap-test] alignment checks passed");

    let layout = Layout::from_size_align(128, 16).expect("valid reuse layout");
    // SAFETY: initialized allocator and valid layout; freed below.
    let first = unsafe { super::allocator::ALLOCATOR.alloc(layout) };
    assert!(!first.is_null());
    // SAFETY: matching live allocation.
    unsafe { super::allocator::ALLOCATOR.dealloc(first, layout) };
    // SAFETY: initialized allocator and valid layout; freed below.
    let second = unsafe { super::allocator::ALLOCATOR.alloc(layout) };
    assert!(!second.is_null());
    // SAFETY: matching live allocation.
    unsafe { super::allocator::ALLOCATOR.dealloc(second, layout) };
    kprintln!("[heap-test] deallocation/reuse passed");

    fragmentation_test();
    kprintln!("[heap-test] fragmentation/coalescing passed");

    #[cfg(all(feature = "heap-oom-test", feature = "qemu-test"))]
    run_oom_test();
    #[cfg(all(feature = "heap-guard-fault-test", feature = "qemu-test"))]
    run_guard_fault_test();

    #[cfg(not(any(
        all(feature = "heap-oom-test", feature = "qemu-test"),
        all(feature = "heap-guard-fault-test", feature = "qemu-test")
    )))]
    kprintln!("[heap-test] completed successfully");
}

fn in_heap(address: usize) -> bool {
    address >= HEAP_START as usize && address < HEAP_END as usize
}

fn fragmentation_test() {
    let layout = Layout::from_size_align(2048, 16).expect("valid fragmentation layout");
    // SAFETY: allocations use a valid layout from the initialized allocator;
    // all are freed below exactly once.
    let a = unsafe { super::allocator::ALLOCATOR.alloc(layout) };
    // SAFETY: same as above.
    let b = unsafe { super::allocator::ALLOCATOR.alloc(layout) };
    // SAFETY: same as above.
    let c = unsafe { super::allocator::ALLOCATOR.alloc(layout) };
    assert!(!a.is_null() && !b.is_null() && !c.is_null());
    // SAFETY: `b` is the live allocation made above.
    unsafe { super::allocator::ALLOCATOR.dealloc(b, layout) };
    // SAFETY: allocator is initialized; this fits in the newly freed region.
    let d = unsafe { super::allocator::ALLOCATOR.alloc(layout) };
    assert!(!d.is_null());
    // SAFETY: each pointer corresponds to its still-live allocation.
    unsafe {
        super::allocator::ALLOCATOR.dealloc(a, layout);
        super::allocator::ALLOCATOR.dealloc(c, layout);
        super::allocator::ALLOCATOR.dealloc(d, layout);
    }
    let large = Layout::from_size_align(HEAP_SIZE / 2, 16).expect("valid coalescing layout");
    // SAFETY: allocator is initialized; all previous allocations have been
    // returned and the free-list coalescing above must make this fit.
    let joined = unsafe { super::allocator::ALLOCATOR.alloc(large) };
    assert!(!joined.is_null(), "coalesced heap region was not reusable");
    assert!(in_heap(joined as usize));
    // SAFETY: matching live allocation.
    unsafe { super::allocator::ALLOCATOR.dealloc(joined, large) };

    let near_capacity =
        Layout::from_size_align(HEAP_SIZE - 16, 16).expect("valid heap-boundary layout");
    // SAFETY: all prior allocations are freed and the fixed heap has enough
    // space for this boundary-sized allocation.
    let boundary = unsafe { super::allocator::ALLOCATOR.alloc(near_capacity) };
    assert!(!boundary.is_null(), "near-capacity heap allocation failed");
    assert!(in_heap(boundary as usize));
    assert!(
        (boundary as usize)
            .checked_add(near_capacity.size())
            .is_some_and(|end| end <= HEAP_END as usize)
    );
    // SAFETY: matching live allocation.
    unsafe { super::allocator::ALLOCATOR.dealloc(boundary, near_capacity) };
}

#[cfg(all(feature = "heap-oom-test", feature = "qemu-test"))]
fn run_oom_test() -> ! {
    let layout = Layout::from_size_align(HEAP_SIZE * 2, 16).expect("valid OOM layout");
    // SAFETY: deliberate oversized request to the initialized allocator.
    let result = unsafe { super::allocator::ALLOCATOR.alloc(layout) };
    assert!(
        result.is_null(),
        "oversized heap allocation unexpectedly succeeded"
    );
    kprintln!("[heap-test] expected allocation failure");
    super::super::qemu::exit_success()
}

#[cfg(all(feature = "heap-guard-fault-test", feature = "qemu-test"))]
fn run_guard_fault_test() -> ! {
    let address = HEAP_START - PAGE_SIZE;
    assert!(with_kernel_address_space(|space| matches!(
        space.translate(VirtAddr::new(address)),
        Translation::NotMapped
    )));
    EXPECTED_GUARD_FAULT_ADDRESS.store(address, Ordering::Relaxed);
    EXPECTED_GUARD_FAULT_ARMED.store(true, Ordering::Relaxed);
    kprintln!("[heap-test] triggering expected heap guard-page fault");
    // SAFETY: deliberate supervisor read from the verified unmapped lower
    // guard page; the feature-specific handler accepts only this #PF.
    unsafe { VirtAddr::new(address).as_ptr::<u64>().read_volatile() };
    panic!("heap guard page access did not fault")
}

#[cfg(all(feature = "heap-guard-fault-test", feature = "qemu-test"))]
pub(crate) fn take_expected_guard_fault(
    address: u64,
    error: x86_64::structures::idt::PageFaultErrorCode,
    cpl: x86_64::PrivilegeLevel,
) -> bool {
    address == EXPECTED_GUARD_FAULT_ADDRESS.load(Ordering::Relaxed)
        && cpl == x86_64::PrivilegeLevel::Ring0
        && error.is_empty()
        && EXPECTED_GUARD_FAULT_ARMED.swap(false, Ordering::Relaxed)
}
