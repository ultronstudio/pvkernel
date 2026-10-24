use core::sync::atomic::{AtomicBool, Ordering};
use x86_64::{
    VirtAddr,
    structures::paging::{Page, PageTableFlags, Size4KiB},
};

use crate::{
    arch::x86_64::memory::{Permissions, Translation, with_kernel_address_space},
    kprintln,
};

pub const MAX_KERNEL_TASKS: usize = 8;
pub const STACK_SIZE: u64 = 16 * 1024;
const PAGE_SIZE: u64 = 4096;
const GUARD_PAGES: u64 = 2;
const SLOT_PAGES: u64 = STACK_SIZE / PAGE_SIZE + GUARD_PAGES;
pub const STACK_REGION_START: u64 = 0xffff_9000_2000_0000;
const STACK_REGION_SIZE: u64 = stack_region_size();
static STACKS_INITIALIZED: AtomicBool = AtomicBool::new(false);

const fn stack_region_size() -> u64 {
    let slot_size = match SLOT_PAGES.checked_mul(PAGE_SIZE) {
        Some(size) => size,
        None => panic!("task stack slot size overflows"),
    };
    match slot_size.checked_mul(MAX_KERNEL_TASKS as u64) {
        Some(size) => size,
        None => panic!("task stack region size overflows"),
    }
}

pub fn init_stacks() {
    let end = STACK_REGION_START
        .checked_add(STACK_REGION_SIZE)
        .expect("task stack virtual region overflow");
    let last_address = end.checked_sub(1).expect("task stack region is empty");
    assert_eq!(STACK_REGION_START % PAGE_SIZE, 0);
    assert!(VirtAddr::try_new(STACK_REGION_START).is_ok());
    assert!(VirtAddr::try_new(last_address).is_ok());
    kprintln!("[scheduler] stack region: {STACK_REGION_START:#x}..{end:#x}");
    with_kernel_address_space(|space| {
        for slot in 0..MAX_KERNEL_TASKS {
            let (low, high) = bounds_unchecked(slot);
            let lower_guard = low
                .checked_sub(PAGE_SIZE)
                .expect("lower stack guard underflow");
            let upper_guard = high;
            for address in (lower_guard..=upper_guard).step_by(PAGE_SIZE as usize) {
                assert!(
                    matches!(
                        space.translate(VirtAddr::new(address)),
                        Translation::NotMapped
                    ),
                    "task stack or guard page already mapped at {address:#x}"
                );
            }
        }
        for slot in 0..MAX_KERNEL_TASKS {
            let (low, high) = bounds_unchecked(slot);
            let middle = low
                .checked_add(STACK_SIZE / 2)
                .expect("middle stack address overflow");
            let last = high
                .checked_sub(PAGE_SIZE)
                .expect("last stack page underflow");
            let pages = usize::try_from(STACK_SIZE / PAGE_SIZE).expect("stack page count overflow");
            for index in 0..pages {
                let offset = u64::try_from(index)
                    .ok()
                    .and_then(|index| index.checked_mul(PAGE_SIZE))
                    .expect("stack page offset overflow");
                let address = low.checked_add(offset).expect("stack page overflow");
                let page = Page::<Size4KiB>::from_start_address(VirtAddr::new(address))
                    .expect("task stack page is unaligned");
                space
                    .map_new_page(page, Permissions::KERNEL_DATA)
                    .unwrap_or_else(|error| {
                        panic!("failed to map task stack page {address:#x}: {error}")
                    });
            }
            for address in [low, middle, last] {
                match space.translate(VirtAddr::new(address)) {
                    Translation::Mapped { flags, .. } => {
                        assert!(flags.contains(
                            PageTableFlags::PRESENT
                                | PageTableFlags::WRITABLE
                                | PageTableFlags::NO_EXECUTE
                        ));
                        assert!(!flags.contains(PageTableFlags::USER_ACCESSIBLE));
                    }
                    other => panic!("task stack mapping missing at {address:#x}: {other:?}"),
                }
            }
            assert!(matches!(
                space.translate(VirtAddr::new(
                    low.checked_sub(PAGE_SIZE)
                        .expect("lower stack guard underflow")
                )),
                Translation::NotMapped
            ));
            assert!(matches!(
                space.translate(VirtAddr::new(high)),
                Translation::NotMapped
            ));
        }
    });
    STACKS_INITIALIZED.store(true, Ordering::Release);
    kprintln!("[scheduler] task stacks verified supervisor RW/NX with guard pages");
}

pub fn stack_bounds(slot: usize) -> Option<(u64, u64)> {
    (slot < MAX_KERNEL_TASKS).then(|| bounds_unchecked(slot))
}

pub fn is_validated_stack_top(top: u64) -> bool {
    STACKS_INITIALIZED.load(Ordering::Acquire)
        && (0..MAX_KERNEL_TASKS).any(|slot| stack_bounds(slot).is_some_and(|(_, high)| high == top))
}

fn bounds_unchecked(slot: usize) -> (u64, u64) {
    let stride = SLOT_PAGES
        .checked_mul(PAGE_SIZE)
        .expect("task stack slot overflow");
    let slot_start = STACK_REGION_START
        .checked_add(
            u64::try_from(slot)
                .expect("task stack slot index does not fit in u64")
                .checked_mul(stride)
                .expect("stack slot offset overflow"),
        )
        .expect("stack slot address overflow");
    let low = slot_start
        .checked_add(PAGE_SIZE)
        .expect("stack low address overflow");
    let high = low
        .checked_add(STACK_SIZE)
        .expect("stack high address overflow");
    (low, high)
}

pub fn verify_stack_mappings() {
    with_kernel_address_space(|space| {
        for slot in 0..MAX_KERNEL_TASKS {
            let (low, high) = bounds_unchecked(slot);
            let lower_guard = low
                .checked_sub(PAGE_SIZE)
                .expect("lower stack guard underflow");
            let middle = low
                .checked_add(STACK_SIZE / 2)
                .expect("middle stack address overflow");
            let last = high
                .checked_sub(PAGE_SIZE)
                .expect("last stack page underflow");
            for address in [lower_guard, low, middle, last, high] {
                let mapped = !matches!(
                    space.translate(VirtAddr::new(address)),
                    Translation::NotMapped
                );
                assert_eq!(
                    mapped,
                    address != lower_guard && address != high,
                    "task stack guard/mapping mismatch at {address:#x}"
                );
            }
        }
    });
}
