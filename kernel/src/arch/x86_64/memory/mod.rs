//! Physical-memory and virtual-memory foundation.
//!
//! This module owns:
//!
//! * [`frame_allocator::BootFrameAllocator`] — a monotonic boot-time
//!   physical frame allocator built from the bootloader's memory map;
//! * [`address_space::KernelAddressSpace`] — the active x86_64 page table,
//!   reachable only through [`with_kernel_address_space`];
//! * [`paging`] — focused 4 KiB map/unmap/permission-change/translate
//!   operations and the [`Permissions`] type.
//!
//! See [docs/architecture/virtual-memory.md](../../../../../docs/architecture/virtual-memory.md)
//! for the full address-space policy, unsafe invariants, and what is
//! deliberately **not** implemented yet.
//!
//! pvKernel 0.0.5 does not execute ring-3 code, does not load pvOS
//! executables, and does not define a stable syscall ABI.

mod address_space;
mod frame_allocator;
mod paging;

#[cfg(feature = "qemu-test")]
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use bootloader_api::BootInfo;
use bootloader_api::info::{MemoryRegion, MemoryRegionKind};
#[cfg(feature = "qemu-test")]
use x86_64::structures::idt::PageFaultErrorCode;
use x86_64::{
    VirtAddr,
    registers::control::{Cr4, Cr4Flags},
    registers::model_specific::{Efer, EferFlags},
    structures::paging::PageTableFlags,
};

pub use address_space::with_kernel_address_space;
pub use paging::{Permissions, Translation};

use frame_allocator::BootFrameAllocator;
use paging::MemoryError;

use crate::kprintln;

/// Base of a reserved, page-aligned virtual range pvKernel uses for
/// temporary and internal test mappings in 0.0.5.
///
/// This range is internal and unstable: no code outside this module's
/// boot-time tests may assume it remains at this address across releases.
/// It is checked as unmapped at runtime immediately before use (see
/// [`run_boot_smoke_test`]), so an unexpected collision with a
/// bootloader-chosen dynamic mapping fails loudly instead of silently
/// corrupting memory.
pub const TEMP_MAPPING_BASE: u64 = 0xFFFF_9000_0000_0000;

/// Base of the reserved userspace virtual-address range.
///
/// Starting in 0.0.6, `arch::x86_64::userspace` maps its (test-only) code,
/// guard, and stack pages within this range; see
/// [docs/architecture/virtual-memory.md](../../../../../docs/architecture/virtual-memory.md)
/// and [docs/architecture/userspace-entry.md](../../../../../docs/architecture/userspace-entry.md).
#[cfg_attr(not(feature = "userspace-test"), allow(dead_code))]
pub const USERSPACE_RANGE_BASE: u64 = 0x0000_6000_0000_0000;

/// Initializes the physical frame allocator and active page-table access.
///
/// Must run after the GDT/IDT are installed (so a page fault during memory
/// initialization is still handled) and while `boot_info` is still
/// available. Only reads `boot_info.physical_memory_offset` and
/// `boot_info.memory_regions`; does not touch `boot_info.framebuffer`, so
/// the caller can keep using that field afterward.
///
/// # Panics
///
/// Panics if the bootloader did not supply a physical-memory offset
/// mapping. `kernel/src/main.rs` requests this mapping explicitly through a
/// custom `BootloaderConfig` (`mappings.physical_memory =
/// Some(Mapping::Dynamic)`), so its absence would indicate a build
/// misconfiguration, not an ordinary runtime condition; none of this
/// module's operations are possible without it, so continuing would be
/// unsafe.
pub fn init(boot_info: &mut BootInfo) {
    let physical_memory_offset = boot_info
        .physical_memory_offset
        .into_option()
        .expect("bootloader did not provide a physical-memory offset mapping");
    let phys_mem_offset = VirtAddr::new(physical_memory_offset);

    // SAFETY: `bootloader_api::MemoryRegions` points at memory the
    // bootloader allocates once and never frees or reuses for the life of
    // the kernel; this is the same guarantee that lets `BootInfo` itself be
    // passed to `kernel_main` as `&'static mut BootInfo`. Reconstructing a
    // `'static` slice here (instead of retaining a second `&'static mut
    // BootInfo` alias alongside `kernel_main`'s own reference) keeps exactly
    // one owner of the full `BootInfo` while still letting the frame
    // allocator hold the region list for the kernel's entire lifetime.
    let regions: &'static [MemoryRegion] = unsafe {
        core::slice::from_raw_parts(
            boot_info.memory_regions.as_ptr(),
            boot_info.memory_regions.len(),
        )
    };

    let usable_regions = regions
        .iter()
        .filter(|region| region.kind == MemoryRegionKind::Usable)
        .count();

    // SAFETY: `regions` is the bootloader-provided memory map reconstructed
    // immediately above; nothing else allocates physical frames from it.
    let frame_allocator = unsafe { BootFrameAllocator::new(regions) };
    let usable_frames = frame_allocator.usable_frame_count();

    address_space::init(phys_mem_offset, frame_allocator);

    kprintln!("[memory] physical frame allocator initialized");
    kprintln!(
        "[memory] usable regions: {}, allocatable 4 KiB frames: {}",
        usable_regions,
        usable_frames
    );
    kprintln!("[memory] active page table initialized");
    kprintln!(
        "[memory] paging mode: {}",
        if five_level_paging_active() {
            "5-level"
        } else {
            "4-level"
        }
    );
    kprintln!(
        "[memory] NX: {}",
        if nx_enabled() {
            "enabled"
        } else {
            "unavailable"
        }
    );
}

/// Reports whether CR4.LA57 (five-level paging) is active. pvKernel does
/// not enable five-level paging itself; the bootloader currently always
/// configures standard 4-level paging.
fn five_level_paging_active() -> bool {
    Cr4::read().contains(Cr4Flags::L5_PAGING)
}

/// Reports whether EFER.NXE (the mechanism that makes the page-table
/// `NO_EXECUTE` bit meaningful) is active. pvKernel's bootloader
/// (`bootloader-x86_64-common`) already enables this before jumping to
/// `kernel_main`; this only verifies that from runtime state rather than
/// assuming it.
fn nx_enabled() -> bool {
    Efer::read().contains(EferFlags::NO_EXECUTE_ENABLE)
}

/// Boot-time verification of the mapping foundation: maps a fresh page in
/// the temporary test range, translates it, writes and reads back a
/// sentinel value, changes it to read-only, unmaps it, and confirms
/// translation reports it as unmapped afterward. Runs on every normal boot,
/// with maskable interrupts still disabled (see `arch::x86_64::init`).
///
/// Panics (rather than silently continuing) if any step does not behave as
/// expected: this test exists specifically to catch a broken mapping
/// foundation before the rest of boot relies on it.
pub fn run_boot_smoke_test() {
    kprintln!("[memory] running temporary mapping smoke test");

    // A non-canonical address (bits 63:47 not uniformly sign-extended from
    // bit 47) must be rejected without panicking, per this module's "never
    // panic on an ordinary address" contract. `TEMP_MAPPING_BASE` has bit
    // 63 set; clearing it alone breaks the sign-extension requirement while
    // still keeping bit 47 set.
    let non_canonical = TEMP_MAPPING_BASE & !(1 << 63);
    assert!(
        matches!(
            paging::canonical_virt_addr(non_canonical),
            Err(MemoryError::NonCanonicalAddress)
        ),
        "a non-canonical address was not rejected"
    );

    let test_addr = paging::canonical_virt_addr(TEMP_MAPPING_BASE)
        .expect("TEMP_MAPPING_BASE is not a canonical address");
    let page = paging::page_at(test_addr).expect("TEMP_MAPPING_BASE is not page-aligned");

    with_kernel_address_space(|space| {
        assert!(
            matches!(space.translate(test_addr), Translation::NotMapped),
            "temporary test page was already mapped before the smoke test ran"
        );

        let frame = space
            .map_new_page(page, Permissions::KERNEL_DATA)
            .expect("failed to map temporary test page");

        match space.translate(test_addr) {
            Translation::Mapped { phys_addr, flags } => {
                assert_eq!(
                    phys_addr,
                    frame.start_address(),
                    "translated address does not match the mapped frame"
                );
                assert!(
                    flags.contains(PageTableFlags::WRITABLE),
                    "KERNEL_DATA mapping is missing the writable flag"
                );
                assert!(
                    flags.contains(PageTableFlags::NO_EXECUTE),
                    "KERNEL_DATA mapping is missing the no-execute flag"
                );
                assert!(
                    !flags.contains(PageTableFlags::USER_ACCESSIBLE),
                    "KERNEL_DATA mapping is unexpectedly user-accessible"
                );
            }
            other => {
                panic!("temporary test page did not translate as an ordinary mapping: {other:?}")
            }
        }

        const SENTINEL: u32 = 0xC0FF_EE42;
        // SAFETY: `page` was just mapped above as present, writable,
        // supervisor-only, and non-executable, backed by a frame this
        // allocator has not handed out to anyone else. The pointer is used
        // only within this closure and not retained past it, so it cannot
        // outlive the mapping established a few lines above.
        unsafe {
            let ptr = test_addr.as_mut_ptr::<u32>();
            ptr.write_volatile(SENTINEL);
            let read_back = ptr.read_volatile();
            assert_eq!(
                read_back, SENTINEL,
                "read-back value does not match what was written"
            );
        }

        space
            .update_page_permissions(page, Permissions::KERNEL_READ_ONLY)
            .expect("failed to change temporary test page to read-only");

        let unmapped_frame = space
            .unmap_page(page)
            .expect("failed to unmap temporary test page");
        assert_eq!(
            unmapped_frame, frame,
            "unmapped frame does not match the originally mapped frame"
        );

        assert!(
            matches!(space.translate(test_addr), Translation::NotMapped),
            "temporary test page still translates as mapped after unmapping"
        );

        kprintln!(
            "[memory] frames: {} usable, {} allocated so far",
            space.usable_frame_count(),
            space.allocated_frame_count()
        );
    });

    kprintln!("[memory] temporary mapping smoke test passed");
}

/// Set while a deliberate, expected post-unmap page fault is in flight, so
/// `arch::x86_64::interrupts::page_fault_handler` can distinguish it from a
/// real page-fault bug. Only meaningful under `qemu-test`, since only that
/// build can act on the distinction (by exiting QEMU successfully).
#[cfg(feature = "qemu-test")]
static EXPECTED_UNMAP_FAULT_ARMED: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "qemu-test")]
static EXPECTED_UNMAP_FAULT_ADDRESS: AtomicU64 = AtomicU64::new(0);

/// Arms the expected post-unmap fault check for `addr`. Only called from
/// [`run_unmap_fault_test`].
#[cfg(all(feature = "qemu-test", feature = "mem-fault-tests"))]
fn arm_expected_unmap_fault(addr: VirtAddr) {
    EXPECTED_UNMAP_FAULT_ADDRESS.store(addr.as_u64(), Ordering::Relaxed);
    EXPECTED_UNMAP_FAULT_ARMED.store(true, Ordering::Relaxed);
}

/// Checks whether a page fault at `addr` with `error_code` exactly matches
/// the fault [`run_unmap_fault_test`] deliberately provoked: not-present
/// (`error_code` has no bits set at all), at the expected address. Disarms
/// itself on a match so it cannot be reused for a later, unrelated fault.
#[cfg(feature = "qemu-test")]
pub(crate) fn take_expected_unmap_fault_if_matches(
    addr: u64,
    error_code: PageFaultErrorCode,
) -> bool {
    if EXPECTED_UNMAP_FAULT_ARMED.load(Ordering::Relaxed)
        && addr == EXPECTED_UNMAP_FAULT_ADDRESS.load(Ordering::Relaxed)
        && error_code.is_empty()
    {
        EXPECTED_UNMAP_FAULT_ARMED.store(false, Ordering::Relaxed);
        true
    } else {
        false
    }
}

/// Deliberately accesses a virtual page after unmapping it, to exercise the
/// page-fault handler's not-present/supervisor/read classification. Only
/// invoked when the `mem-fault-tests` feature is enabled; never run during
/// normal boot.
///
/// Maps a fresh page in the temporary test range, writes and verifies a
/// sentinel value, unmaps it, then deliberately reads the now-unmapped
/// address. Under `qemu-test`, the page-fault handler recognizes this exact
/// address/error-code combination and exits QEMU successfully instead of
/// treating it as fatal; without `qemu-test`, the fault is fatal and the
/// kernel halts, matching the existing `fault-tests` page-fault test's
/// behavior.
#[cfg(feature = "mem-fault-tests")]
pub fn run_unmap_fault_test() {
    kprintln!("[memory] running post-unmap page-fault test");

    let test_addr = VirtAddr::new(TEMP_MAPPING_BASE);
    let page = paging::page_at(test_addr).expect("TEMP_MAPPING_BASE is not page-aligned");

    with_kernel_address_space(|space| {
        space
            .map_new_page(page, Permissions::KERNEL_DATA)
            .expect("failed to map temporary test page");

        const SENTINEL: u32 = 0x1EE7_0001;
        // SAFETY: same reasoning as `run_boot_smoke_test`: the page was just
        // mapped present/writable/supervisor/non-executable, and the
        // pointer does not outlive this closure.
        unsafe {
            let ptr = test_addr.as_mut_ptr::<u32>();
            ptr.write_volatile(SENTINEL);
            assert_eq!(
                ptr.read_volatile(),
                SENTINEL,
                "read-back value does not match what was written"
            );
        }

        space
            .unmap_page(page)
            .expect("failed to unmap temporary test page");
    });

    #[cfg(feature = "qemu-test")]
    arm_expected_unmap_fault(test_addr);

    kprintln!(
        "[memory] WARNING: deliberately accessing unmapped page {:#x}",
        test_addr.as_u64()
    );

    // SAFETY: this read is not memory-safe; it is an intentional fault used
    // only to exercise the page-fault handler's not-present/supervisor/read
    // classification. `test_addr` was just unmapped above and armed as the
    // expected fault address. The handler never returns to this call site
    // (it exits QEMU under `qemu-test`, or halts otherwise), so the invalid
    // read never produces a usable value.
    unsafe {
        core::ptr::read_volatile(test_addr.as_ptr::<u32>());
    }

    // Reaching this point means no fault occurred, which is itself a test
    // failure: the page should not still be accessible after being unmapped.
    panic!("expected page fault after unmapping did not occur");
}
