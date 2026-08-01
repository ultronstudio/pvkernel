//! Ring-3 (CPL3) entry: mapping a tiny hand-written test program into a
//! dedicated userspace virtual-address range and transitioning into it via
//! `iretq`.
//!
//! Everything in this module is test-only scaffolding, gated behind the
//! `userspace-test` Cargo feature and the three specific
//! `userspace-*-test` features that each imply it: pvKernel 0.0.6 has no
//! process model, no ELF loader, and no scheduler, so there is no "real"
//! user program to run yet. See
//! [docs/architecture/userspace-entry.md](../../../../../docs/architecture/userspace-entry.md).

#[cfg(feature = "userspace-test")]
mod enter;
#[cfg(feature = "userspace-test")]
mod payload;

#[cfg(feature = "userspace-test")]
use x86_64::VirtAddr;
#[cfg(feature = "userspace-test")]
use x86_64::structures::paging::{Page, Size4KiB};

#[cfg(feature = "userspace-test")]
use super::memory::{self, Permissions, Translation};
#[cfg(feature = "userspace-test")]
use crate::kprintln;

#[cfg(all(
    feature = "userspace-syscall-test",
    feature = "userspace-privilege-fault-test"
))]
compile_error!("userspace-syscall-test and userspace-privilege-fault-test are mutually exclusive");
#[cfg(all(
    feature = "userspace-syscall-test",
    feature = "userspace-supervisor-fault-test"
))]
compile_error!("userspace-syscall-test and userspace-supervisor-fault-test are mutually exclusive");
#[cfg(all(
    feature = "userspace-privilege-fault-test",
    feature = "userspace-supervisor-fault-test"
))]
compile_error!(
    "userspace-privilege-fault-test and userspace-supervisor-fault-test are mutually exclusive"
);
#[cfg(all(
    feature = "userspace-test",
    not(feature = "userspace-syscall-test"),
    not(feature = "userspace-privilege-fault-test"),
    not(feature = "userspace-supervisor-fault-test")
))]
compile_error!(
    "userspace-test must be enabled through exactly one of userspace-syscall-test, \
     userspace-privilege-fault-test, or userspace-supervisor-fault-test"
);

/// Base of pvKernel's reserved userspace virtual-address range.
#[cfg(feature = "userspace-test")]
const RANGE_BASE: u64 = memory::USERSPACE_RANGE_BASE;

/// The user code page: holds whichever test payload is selected.
#[cfg(feature = "userspace-test")]
const CODE_ADDR: u64 = RANGE_BASE;
/// Deliberately left unmapped: a guard page immediately after the code
/// page, so a code-page overrun faults instead of silently continuing
/// into whatever happens to be mapped next.
#[cfg(feature = "userspace-test")]
const GUARD_ADDR: u64 = RANGE_BASE + 0x1000;
/// The user stack page.
#[cfg(feature = "userspace-test")]
const STACK_ADDR: u64 = RANGE_BASE + 0x2000;

/// A fixed, kernel-chosen supervisor-only probe address used by the
/// `userspace-supervisor-fault-test` payload (baked into that payload as a
/// compile-time immediate; see `payload`'s matching compile-time
/// assertion). Deliberately distinct from `memory::TEMP_MAPPING_BASE`
/// itself, which other boot-time tests also map transiently.
#[cfg(feature = "userspace-supervisor-fault-test")]
pub(crate) const SUPERVISOR_PROBE_ADDR: u64 = memory::TEMP_MAPPING_BASE + 0x2000;

/// Runs whichever single userspace test scenario is selected by Cargo
/// features (exactly one of the three `userspace-*-test` features, per
/// the `compile_error!` guards above). Never returns: every scenario ends
/// by exiting QEMU (under `qemu-test`) or halting/panicking - see
/// `syscall::test::test_exit` and the fault handlers in
/// `arch::x86_64::interrupts`.
#[cfg(feature = "userspace-test")]
pub fn run_test() -> ! {
    #[cfg(feature = "userspace-syscall-test")]
    {
        run_syscall_test()
    }
    #[cfg(feature = "userspace-privilege-fault-test")]
    {
        run_privilege_fault_test()
    }
    #[cfg(feature = "userspace-supervisor-fault-test")]
    {
        run_supervisor_fault_test()
    }
}

#[cfg(feature = "userspace-syscall-test")]
fn run_syscall_test() -> ! {
    kprintln!("[userspace] preparing syscall-boundary test payload");
    let (entry_rip, stack_top) = map_test_program(payload::syscall_test_payload());
    kprintln!(
        "[userspace] user code page mapped RX+U at {:#x}, stack RW+NX+U at {:#x}",
        CODE_ADDR,
        STACK_ADDR
    );
    kprintln!("[userspace] entering CPL3 at {:#x}", entry_rip.as_u64());

    // SAFETY: `entry_rip`/`stack_top` were just computed by
    // `map_test_program`, which mapped exactly those addresses with the
    // required permissions immediately above and copied the payload
    // returned by `payload::syscall_test_payload` into the code page.
    unsafe { enter::enter_user_mode(entry_rip, stack_top) }
}

#[cfg(feature = "userspace-privilege-fault-test")]
fn run_privilege_fault_test() -> ! {
    kprintln!("[userspace] preparing privilege-fault test payload");
    let (entry_rip, stack_top) = map_test_program(payload::privilege_fault_payload());
    super::interrupts::arm_expected_privilege_fault();
    kprintln!("[userspace] entering CPL3 at {:#x}", entry_rip.as_u64());

    // SAFETY: see `run_syscall_test`; identical reasoning.
    unsafe { enter::enter_user_mode(entry_rip, stack_top) }
}

#[cfg(feature = "userspace-supervisor-fault-test")]
fn run_supervisor_fault_test() -> ! {
    kprintln!("[userspace] preparing supervisor-page-fault test payload");
    map_supervisor_probe_page();
    let (entry_rip, stack_top) = map_test_program(payload::supervisor_fault_payload());
    super::interrupts::arm_expected_supervisor_fault(SUPERVISOR_PROBE_ADDR);
    kprintln!("[userspace] entering CPL3 at {:#x}", entry_rip.as_u64());

    // SAFETY: see `run_syscall_test`; identical reasoning.
    unsafe { enter::enter_user_mode(entry_rip, stack_top) }
}

/// Maps and populates the user code page and user stack page (verifying
/// the guard page stays unmapped) for a deterministic userspace test.
/// Returns the user code entry point and the initial user stack pointer.
///
/// # Panics
///
/// Panics (rather than silently continuing) if any expected precondition
/// does not hold - unmapped target addresses beforehand, a payload that
/// fits in one page, successful mapping - mirroring
/// `memory::run_boot_smoke_test`'s "never silently continue on a broken
/// foundation" policy.
#[cfg(feature = "userspace-test")]
fn map_test_program(payload: &[u8]) -> (VirtAddr, VirtAddr) {
    assert!(
        !payload.is_empty() && payload.len() <= 4096,
        "user test payload must fit within one 4 KiB page"
    );

    // `CODE_ADDR`/`GUARD_ADDR`/`STACK_ADDR` are fixed, compile-time-known,
    // page-aligned, low-half canonical constants (see `RANGE_BASE`), not
    // user input, so constructing `VirtAddr`/`Page` from them directly
    // (rather than through the fallible helpers `memory`'s boot-time tests
    // use) is appropriate here.
    let code_addr = VirtAddr::new(CODE_ADDR);
    let guard_addr = VirtAddr::new(GUARD_ADDR);
    let stack_addr = VirtAddr::new(STACK_ADDR);

    let code_page =
        Page::<Size4KiB>::from_start_address(code_addr).expect("CODE_ADDR is not 4 KiB aligned");
    let stack_page =
        Page::<Size4KiB>::from_start_address(stack_addr).expect("STACK_ADDR is not 4 KiB aligned");

    memory::with_kernel_address_space(|space| {
        assert!(
            matches!(space.translate(code_addr), Translation::NotMapped),
            "user code page is already mapped"
        );
        assert!(
            matches!(space.translate(guard_addr), Translation::NotMapped),
            "user guard page is already mapped"
        );
        assert!(
            matches!(space.translate(stack_addr), Translation::NotMapped),
            "user stack page is already mapped"
        );

        // W^X-safe population sequence: map writable + non-executable
        // first, copy the payload in, then switch to read-only +
        // executable. The page is never both writable and executable at
        // once - `Permissions::to_flags` rejects that combination
        // outright.
        space
            .map_new_page(code_page, Permissions::USER_DATA)
            .expect("failed to map user code page");

        // SAFETY: `code_page` was just mapped above as present, writable,
        // user-accessible, and non-executable, backed by a fresh frame
        // this allocator has not handed out to anyone else. `payload` is a
        // kernel-owned `&[u8]` slice from a supervisor-only
        // `.rodata`-like section (see `payload`); the write stays within
        // the single page just mapped, since `payload.len() <= 4096` was
        // asserted above.
        unsafe {
            core::ptr::copy_nonoverlapping(
                payload.as_ptr(),
                code_addr.as_mut_ptr::<u8>(),
                payload.len(),
            );
        }

        space
            .update_page_permissions(code_page, Permissions::USER_CODE)
            .expect("failed to finalize user code page permissions");

        space
            .map_new_page(stack_page, Permissions::USER_DATA)
            .expect("failed to map user stack page");

        // The guard page must still be unmapped: it is never mapped at
        // all, by design, rather than mapped-then-unmapped.
        assert!(
            matches!(space.translate(guard_addr), Translation::NotMapped),
            "user guard page ended up mapped unexpectedly"
        );
    });

    let entry_rip = code_addr;
    // Top of the mapped stack page; already 16-byte aligned (a 4 KiB
    // aligned address is trivially 16-byte aligned too).
    let stack_top = stack_addr + 0x1000u64;

    (entry_rip, stack_top)
}

/// Maps a present, writable, supervisor-only (**not** user-accessible)
/// page at [`SUPERVISOR_PROBE_ADDR`], for the supervisor-page-fault test
/// to deliberately read from CPL3.
#[cfg(feature = "userspace-supervisor-fault-test")]
fn map_supervisor_probe_page() {
    let probe_addr = VirtAddr::new(SUPERVISOR_PROBE_ADDR);
    let probe_page = Page::<Size4KiB>::from_start_address(probe_addr)
        .expect("SUPERVISOR_PROBE_ADDR is not 4 KiB aligned");

    memory::with_kernel_address_space(|space| {
        assert!(
            matches!(space.translate(probe_addr), Translation::NotMapped),
            "supervisor probe page is already mapped"
        );

        space
            .map_new_page(probe_page, Permissions::KERNEL_DATA)
            .expect("failed to map supervisor probe page");
    });
}
