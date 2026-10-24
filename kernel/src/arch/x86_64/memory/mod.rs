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
mod allocator;
mod frame_allocator;
mod heap;
mod paging;

#[cfg(feature = "qemu-test")]
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use bootloader_api::BootInfo;
use bootloader_api::info::{MemoryRegion, MemoryRegionKind, MemoryRegions};
#[cfg(feature = "qemu-test")]
use x86_64::structures::idt::PageFaultErrorCode;
use x86_64::{
    VirtAddr,
    registers::control::{Cr0, Cr0Flags, Cr4, Cr4Flags},
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

/// First accepted address for process mappings; the first 64 KiB stays unmapped.
pub const USER_MIN: u64 = 0x0000_0000_0001_0000;
/// Exclusive upper bound for the lower canonical userspace half.
pub const USER_MAX: u64 = 0x0000_8000_0000_0000;

/// PML4 index 256: first upper-half slot reserved for bootloader dynamics.
pub const BOOTLOADER_DYNAMIC_START: u64 = 0xffff_8000_0000_0000;
/// Exclusive boundary before PML4 index 288, where pvKernel's explicit
/// scratch, heap, and task-stack mappings begin.
pub const BOOTLOADER_DYNAMIC_END: u64 = 0xffff_9000_0000_0000;
/// The bootloader rounds its dynamic-range end to PML4 entries. This page
/// sentinel makes it reserve index 288 onward and leave indices 256..287.
pub const BOOTLOADER_DYNAMIC_END_SENTINEL: u64 = BOOTLOADER_DYNAMIC_END - 4096;

/// Placeholder [`MemoryRegions`] used only to give
/// `BootInfo::memory_regions` a valid value to hold once [`init`] has moved
/// the real one out of it (see [`init`]). Always empty, and never read
/// through: `init` runs exactly once, consumes `boot_info` completely, and
/// does not return it to its caller, so nothing ever observes this
/// placeholder as if it were real memory-map data.
static mut EMPTY_MEMORY_REGIONS: [MemoryRegion; 0] = [];

/// Initializes the physical frame allocator and active page-table access.
///
/// Must run after the GDT/IDT are installed (so a page fault during memory
/// initialization is still handled).
///
/// Takes `boot_info` by value and does not return it: this function (via
/// [`arch::x86_64::init`](super::init), which forwards `boot_info` here
/// unchanged) is the last code that ever touches `BootInfo`. It moves the
/// real [`MemoryRegions`] out of `boot_info.memory_regions` (see below)
/// instead of reconstructing an aliasing `'static` view of it, so once this
/// function returns there is no live `&mut BootInfo` anywhere in the
/// kernel, and the physical frame allocator's memory-map reference is the
/// *sole* owner of that data rather than an alias of a field someone else
/// could still mutate.
///
/// # Panics
///
/// Panics if the CPU/boot environment has not enabled EFER.NXE, or if
/// CR4.LA57 (five-level paging) is active. pvKernel's paging security model
/// depends on NX, and every later step assumes a four-level CR3 root. CR0.WP
/// is enabled and verified here before page-table initialization.
///
/// Panics if the bootloader did not supply a physical-memory offset
/// mapping. `kernel/src/main.rs` requests this mapping explicitly through a
/// custom `BootloaderConfig` (`mappings.physical_memory =
/// Some(Mapping::Dynamic)`), so its absence would indicate a build
/// misconfiguration, not an ordinary runtime condition; none of this
/// module's operations are possible without it, so continuing would be
/// unsafe.
pub fn init(boot_info: &'static mut BootInfo, framebuffer_mapping: Option<(u64, u64)>) {
    // Paging-mode prerequisites must hold before anything below interprets
    // CR3 as a four-level table root (`address_space::init` does, via
    // `active_level_4_table`/`OffsetPageTable::new`).
    validate_paging_prerequisites();

    let physical_memory_offset = boot_info
        .physical_memory_offset
        .into_option()
        .expect("bootloader did not provide a physical-memory offset mapping");
    let phys_mem_offset = VirtAddr::new(physical_memory_offset);
    let max_physical_address = boot_info
        .memory_regions
        .iter()
        .map(|region| region.end)
        .max()
        .unwrap_or(0)
        .max(0x1_0000_0000);
    let memory_regions_start = boot_info.memory_regions.as_ptr() as u64;
    let memory_regions_len = boot_info
        .memory_regions
        .len()
        .checked_mul(core::mem::size_of::<MemoryRegion>())
        .and_then(|bytes| u64::try_from(bytes).ok())
        .expect("boot memory-map byte length overflow");
    let ramdisk = boot_info
        .ramdisk_addr
        .into_option()
        .map(|start| address_space::MappingRange {
            start,
            len: boot_info.ramdisk_len,
        });
    let boot_mappings = address_space::BootMappingInventory {
        kernel_image_base: boot_info.kernel_image_offset,
        kernel_elf_physical: boot_info.kernel_addr,
        kernel_elf_len: boot_info.kernel_len,
        kernel_stack: address_space::MappingRange {
            start: boot_info.kernel_stack_bottom,
            len: boot_info.kernel_stack_len,
        },
        boot_info: address_space::MappingRange {
            start: boot_info as *mut BootInfo as u64,
            len: core::mem::size_of::<BootInfo>() as u64,
        },
        memory_regions: address_space::MappingRange {
            start: memory_regions_start,
            len: memory_regions_len,
        },
        physical_memory: address_space::MappingRange {
            start: physical_memory_offset,
            len: max_physical_address,
        },
        framebuffer: framebuffer_mapping
            .map(|(start, len)| address_space::MappingRange { start, len }),
        ramdisk,
    };

    // Moves the real `MemoryRegions` out of `boot_info`, leaving the
    // always-empty placeholder in its place so the field stays valid. This
    // is an ordinary, structurally-enforced ownership transfer, not an
    // alias: after this line, `boot_info.memory_regions` no longer refers
    // to the bootloader's actual memory map at all, so even the (already
    // unreachable, since `boot_info` is never returned) old owner could not
    // observe or mutate the data `regions` below now exclusively owns.
    //
    // SAFETY: `EMPTY_MEMORY_REGIONS` is a zero-length array, so the
    // `&'static mut` reference taken from it can never be used to read or
    // write any memory; forming it is sound no matter how many times this
    // runs. (In practice it runs exactly once: `init` is called exactly
    // once, from `arch::x86_64::init`, which is itself called exactly once,
    // from the bootloader entry point.)
    let placeholder = MemoryRegions::from(unsafe { &mut EMPTY_MEMORY_REGIONS[..] });
    let regions: MemoryRegions = core::mem::replace(&mut boot_info.memory_regions, placeholder);
    let regions: &'static mut [MemoryRegion] = regions.into();
    // Downgraded to a shared slice immediately: nothing below ever needs to
    // mutate the memory map, so this removes the ability to, rather than
    // merely documenting that nothing does.
    let regions: &'static [MemoryRegion] = regions;

    let usable_regions = regions
        .iter()
        .filter(|region| region.kind == MemoryRegionKind::Usable)
        .count();

    // SAFETY: `regions` was just moved out of the bootloader-provided
    // `BootInfo` above (not aliased from it), so it is the sole owner of
    // this memory map; nothing else allocates physical frames from it.
    let frame_allocator = unsafe { BootFrameAllocator::new(regions) };
    let usable_frames = frame_allocator.usable_frame_count();

    address_space::init(phys_mem_offset, frame_allocator, boot_mappings);

    kprintln!("[memory] physical frame allocator initialized");
    kprintln!(
        "[memory] usable regions: {}, allocatable 4 KiB frames: {}",
        usable_regions,
        usable_frames
    );
    kprintln!("[memory] active page table initialized");
    kprintln!("[memory] paging mode: 4-level (validated before address-space init)");
    kprintln!(
        "[memory] NX: {}",
        if nx_enabled() {
            "enabled"
        } else {
            "unavailable"
        }
    );
    heap::init();
}

/// Runs heap-backed `Box`, `Vec`, `String`, alignment, reuse, and
/// fragmentation smoke tests after heap initialization.
pub fn run_heap_smoke_tests() {
    #[cfg(feature = "qemu-test")]
    frame_allocator::run_cursor_regression_test();
    heap::run_smoke_tests();
}

#[cfg(all(feature = "heap-guard-fault-test", feature = "qemu-test"))]
pub(crate) fn take_expected_heap_guard_fault(
    address: u64,
    error_code: PageFaultErrorCode,
    cpl: x86_64::PrivilegeLevel,
) -> bool {
    heap::take_expected_guard_fault(address, error_code, cpl)
}

/// Verifies the paging-mode precondition every later step in [`init`]
/// assumes: that the active page table is a standard four-level x86_64
/// hierarchy, not the five-level (LA57) variant. Must run before
/// `address_space::init` ever interprets CR3 as the root of a four-level
/// table; those functions assume four-level paging but do not themselves
/// check it, so this check cannot be deferred until after they run.
///
/// # Panics
///
/// Panics if CR4.LA57 is set. pvKernel does not implement five-level
/// paging; the bootloader currently always configures standard four-level
/// paging, but this is verified rather than assumed, so a misconfigured or
/// future bootloader that enabled LA57 could not cause every later
/// page-table walk to be silently misinterpreted.
fn validate_paging_prerequisites() {
    assert!(
        !five_level_paging_active(),
        "pvKernel does not support 5-level paging (CR4.LA57 is set)"
    );
    assert!(
        nx_enabled(),
        "pvKernel requires execute-disable support with IA32_EFER.NXE enabled"
    );

    // Supervisor write protection is required for read-only page-table
    // permissions to constrain kernel code. Without CR0.WP, supervisor
    // writes can bypass those mappings and undermine page protections and
    // future kernel hardening. Own this invariant rather than assuming the
    // bootloader left it enabled, then verify the resulting architectural
    // state before any memory-management operation proceeds.
    let cr0 = Cr0::read();
    if !cr0.contains(Cr0Flags::WRITE_PROTECT) {
        // SAFETY: setting CR0.WP is architecturally valid in long mode and
        // enables supervisor write protection; it does not alter paging
        // mode or the currently active address-space root.
        unsafe { Cr0::write(cr0 | Cr0Flags::WRITE_PROTECT) };
    }
    assert!(
        Cr0::read().contains(Cr0Flags::WRITE_PROTECT),
        "pvKernel requires CR0.WP supervisor write protection enabled"
    );
}

/// Reports whether CR4.LA57 (five-level paging) is active. pvKernel does
/// not enable five-level paging itself; the bootloader currently always
/// configures standard 4-level paging. Used by
/// [`validate_paging_prerequisites`], which turns this into a hard
/// precondition instead of an informational log.
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

/// Maps a supervisor-only read-only page and deliberately writes to it from
/// CPL0. The page-fault handler accepts only the exact expected protection
/// violation; returning from the write is a test failure. Run with
/// `supervisor-write-protection-test`, and combine with `qemu-test` for a
/// deterministic successful QEMU exit.
#[cfg(feature = "supervisor-write-protection-test")]
pub fn run_supervisor_write_protection_test() {
    kprintln!("[memory] running supervisor write-protection test");

    let test_addr = VirtAddr::new(TEMP_MAPPING_BASE);
    let page = paging::page_at(test_addr).expect("TEMP_MAPPING_BASE is not page-aligned");

    with_kernel_address_space(|space| {
        assert!(
            matches!(space.translate(test_addr), Translation::NotMapped),
            "temporary test page was already mapped before write-protection test"
        );

        space
            .map_new_page(page, Permissions::KERNEL_READ_ONLY)
            .expect("failed to map supervisor read-only test page");

        match space.translate(test_addr) {
            Translation::Mapped { flags, .. } => {
                assert!(flags.contains(PageTableFlags::PRESENT));
                assert!(!flags.contains(PageTableFlags::WRITABLE));
                assert!(!flags.contains(PageTableFlags::USER_ACCESSIBLE));
                assert!(flags.contains(PageTableFlags::NO_EXECUTE));
            }
            other => panic!("read-only test page did not map: {other:?}"),
        }
    });

    super::interrupts::arm_expected_write_protection_fault(test_addr.as_u64());
    kprintln!(
        "[memory] deliberately writing to supervisor read-only page {:#x}",
        test_addr.as_u64()
    );

    // SAFETY: the mapping was verified present, supervisor-only, read-only,
    // and non-executable immediately above. This volatile write is
    // intentionally invalid and must be intercepted by the page-fault
    // handler; returning means CR0.WP or page protection failed.
    unsafe {
        test_addr.as_mut_ptr::<u32>().write_volatile(0x5750_0006);
    }

    panic!("supervisor write to a read-only page did not fault");
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
