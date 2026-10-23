#[cfg(any(feature = "qemu-test", feature = "userspace-syscall-test"))]
use core::sync::atomic::Ordering;
#[cfg(feature = "qemu-test")]
use core::sync::atomic::{AtomicBool, AtomicU64};

use spin::Once;
use x86_64::VirtAddr;
use x86_64::registers::control::Cr2;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

use super::gdt::DOUBLE_FAULT_IST_INDEX;
#[cfg(feature = "qemu-test")]
use super::memory;
use super::pic::{self, IrqLine};
#[cfg(feature = "qemu-test")]
use super::qemu;
#[cfg(not(feature = "qemu-test"))]
use crate::halt::halt_forever;
use crate::kprintln;

static IDT: Once<InterruptDescriptorTable> = Once::new();

/// Installs the pvKernel IDT and loads it onto the current CPU.
///
/// Must run after [`super::gdt::init`], because the double-fault entry below
/// is only safe once the corresponding IST stack is active in the TSS. Also
/// installs handlers for the PIC's remapped IRQ vectors 32-47, so that every
/// vector has a valid handler before the PIC or PIT are ever configured.
pub fn init() {
    kprintln!("[cpu] loading IDT");

    let idt = IDT.call_once(|| {
        let mut idt = InterruptDescriptorTable::new();

        idt.breakpoint.set_handler_fn(breakpoint_handler);
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        idt.general_protection_fault
            .set_handler_fn(general_protection_fault_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);

        // SAFETY: DOUBLE_FAULT_IST_INDEX names an IST entry that gdt::init
        // has already populated with the dedicated double-fault stack, and no
        // other handler is assigned this IST index, so switching to it here
        // is valid and exclusive to this handler.
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(DOUBLE_FAULT_IST_INDEX);
        }

        idt[IrqLine::Timer.vector()].set_handler_fn(timer_interrupt_handler);
        idt[IrqLine::Keyboard.vector()].set_handler_fn(irq1_handler);
        idt[IrqLine::Cascade.vector()].set_handler_fn(irq2_handler);
        idt[IrqLine::Com2.vector()].set_handler_fn(irq3_handler);
        idt[IrqLine::Com1.vector()].set_handler_fn(irq4_handler);
        idt[IrqLine::Lpt2.vector()].set_handler_fn(irq5_handler);
        idt[IrqLine::Floppy.vector()].set_handler_fn(irq6_handler);
        idt[IrqLine::Lpt1.vector()].set_handler_fn(master_spurious_interrupt_handler);
        idt[IrqLine::RealTimeClock.vector()].set_handler_fn(irq8_handler);
        idt[IrqLine::Free9.vector()].set_handler_fn(irq9_handler);
        idt[IrqLine::Free10.vector()].set_handler_fn(irq10_handler);
        idt[IrqLine::Free11.vector()].set_handler_fn(irq11_handler);
        idt[IrqLine::Mouse.vector()].set_handler_fn(irq12_handler);
        idt[IrqLine::FpuCoprocessor.vector()].set_handler_fn(irq13_handler);
        idt[IrqLine::PrimaryAta.vector()].set_handler_fn(irq14_handler);
        idt[IrqLine::SecondaryAta.vector()].set_handler_fn(slave_spurious_interrupt_handler);

        // SAFETY: `pvkernel_syscall_entry_stub` is a naked-asm function
        // (see `syscall::entry`) that builds its own valid `iretq`-based
        // return sequence instead of following the `x86-interrupt`
        // calling convention `set_handler_fn` expects, which is why this
        // uses the raw `set_handler_addr` API. Its address is a plain
        // function pointer to code that exists for the kernel's entire
        // lifetime, and `set_privilege_level(Ring3)` is required for the
        // gate to be reachable by a CPL3 `int 0x80` at all (the default
        // gate DPL only permits CPL0).
        unsafe {
            idt[0x80]
                .set_handler_addr(VirtAddr::new(
                    super::syscall::pvkernel_syscall_entry_stub as *const () as u64,
                ))
                .set_privilege_level(x86_64::PrivilegeLevel::Ring3);
        }

        idt
    });

    idt.load();

    kprintln!("[cpu] IDT loaded");
    kprintln!("[cpu] int 0x80 syscall gate installed (ring 3 callable)");
}

/// Triggers a `int3` breakpoint exception as a boot-time verification that
/// the IDT is installed and that execution correctly resumes afterward.
pub fn run_breakpoint_test() {
    kprintln!("[test] triggering breakpoint");
    x86_64::instructions::interrupts::int3();
    kprintln!("[test] breakpoint returned successfully");
}

/// Set while a deliberate, expected page fault is in flight, so the handler
/// can distinguish it from a real page-fault bug. Only meaningful under
/// `qemu-test`, since only that build can act on the distinction (by exiting
/// QEMU successfully).
#[cfg(feature = "qemu-test")]
static EXPECTED_PAGE_FAULT: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "qemu-test")]
static EXPECTED_PAGE_FAULT_ADDRESS: AtomicU64 = AtomicU64::new(0);

/// Set while the `userspace-privilege-fault-test` scenario's deliberate
/// CPL3 `cli` fault is in flight. Only meaningful under `qemu-test`; without
/// it, the fault is fatal and the kernel halts, matching the existing
/// `fault-tests`/`mem-fault-tests` convention (see
/// `memory::run_unmap_fault_test`).
#[cfg(all(feature = "qemu-test", feature = "userspace-privilege-fault-test"))]
static EXPECTED_PRIVILEGE_FAULT: AtomicBool = AtomicBool::new(false);

/// Arms the expected privilege-fault check. Only called from
/// `userspace::run_privilege_fault_test`, immediately before entering CPL3.
/// A no-op without `qemu-test` (see [`EXPECTED_PRIVILEGE_FAULT`]).
#[cfg(feature = "userspace-privilege-fault-test")]
pub(crate) fn arm_expected_privilege_fault() {
    #[cfg(feature = "qemu-test")]
    EXPECTED_PRIVILEGE_FAULT.store(true, Ordering::Relaxed);
}

/// Checks whether a `#GP` observed at `cpl` exactly matches the fault
/// [`arm_expected_privilege_fault`] deliberately armed: it must have
/// originated at CPL3 (a CPL0 `#GP` is never this test's expected fault,
/// even while armed - that would be a real kernel bug). Disarms itself on a
/// match so it cannot be reused for a later, unrelated fault.
#[cfg(all(feature = "qemu-test", feature = "userspace-privilege-fault-test"))]
fn take_expected_privilege_fault_if_matches(cpl: x86_64::PrivilegeLevel) -> bool {
    if EXPECTED_PRIVILEGE_FAULT.load(Ordering::Relaxed) && cpl == x86_64::PrivilegeLevel::Ring3 {
        EXPECTED_PRIVILEGE_FAULT.store(false, Ordering::Relaxed);
        true
    } else {
        false
    }
}

/// Set while the `userspace-supervisor-fault-test` scenario's deliberate
/// CPL3 read of a supervisor-only page is in flight. Only meaningful under
/// `qemu-test`; see [`EXPECTED_PRIVILEGE_FAULT`] for why.
#[cfg(all(feature = "qemu-test", feature = "userspace-supervisor-fault-test"))]
static EXPECTED_SUPERVISOR_FAULT_ARMED: AtomicBool = AtomicBool::new(false);
#[cfg(all(feature = "qemu-test", feature = "userspace-supervisor-fault-test"))]
static EXPECTED_SUPERVISOR_FAULT_ADDRESS: AtomicU64 = AtomicU64::new(0);

/// Set while a deliberate CPL0 write to a read-only page is in flight.
#[cfg(all(feature = "qemu-test", feature = "supervisor-write-protection-test"))]
static EXPECTED_WRITE_PROTECTION_FAULT_ARMED: AtomicBool = AtomicBool::new(false);
#[cfg(all(feature = "qemu-test", feature = "supervisor-write-protection-test"))]
static EXPECTED_WRITE_PROTECTION_FAULT_ADDRESS: AtomicU64 = AtomicU64::new(0);

/// Arms the expected supervisor write-protection fault immediately before
/// the test's deliberate CPL0 write. A no-op without `qemu-test`.
#[cfg(feature = "supervisor-write-protection-test")]
pub(crate) fn arm_expected_write_protection_fault(addr: u64) {
    #[cfg(feature = "qemu-test")]
    {
        EXPECTED_WRITE_PROTECTION_FAULT_ADDRESS.store(addr, Ordering::Relaxed);
        EXPECTED_WRITE_PROTECTION_FAULT_ARMED.store(true, Ordering::Relaxed);
    }
    #[cfg(not(feature = "qemu-test"))]
    let _ = addr;
}

/// Matches only a present-page write protection fault at the expected
/// address, caused by supervisor code and not an instruction fetch.
#[cfg(all(feature = "qemu-test", feature = "supervisor-write-protection-test"))]
fn take_expected_write_protection_fault_if_matches(
    addr: u64,
    cpl: x86_64::PrivilegeLevel,
    error_code: PageFaultErrorCode,
) -> bool {
    if EXPECTED_WRITE_PROTECTION_FAULT_ARMED.load(Ordering::Relaxed)
        && addr == EXPECTED_WRITE_PROTECTION_FAULT_ADDRESS.load(Ordering::Relaxed)
        && cpl == x86_64::PrivilegeLevel::Ring0
        && error_code.contains(PageFaultErrorCode::PROTECTION_VIOLATION)
        && error_code.contains(PageFaultErrorCode::CAUSED_BY_WRITE)
        && !error_code.contains(PageFaultErrorCode::USER_MODE)
        && !error_code.contains(PageFaultErrorCode::INSTRUCTION_FETCH)
    {
        EXPECTED_WRITE_PROTECTION_FAULT_ARMED.store(false, Ordering::Relaxed);
        true
    } else {
        false
    }
}

/// Arms the expected supervisor-page-fault check for `addr`. Only called
/// from `userspace::run_supervisor_fault_test`, immediately before entering
/// CPL3. A no-op without `qemu-test`.
#[cfg(feature = "userspace-supervisor-fault-test")]
pub(crate) fn arm_expected_supervisor_fault(addr: u64) {
    #[cfg(feature = "qemu-test")]
    {
        EXPECTED_SUPERVISOR_FAULT_ADDRESS.store(addr, Ordering::Relaxed);
        EXPECTED_SUPERVISOR_FAULT_ARMED.store(true, Ordering::Relaxed);
    }
    #[cfg(not(feature = "qemu-test"))]
    let _ = addr;
}

/// Checks whether a `#PF` at `addr` with `error_code` exactly matches the
/// fault [`arm_expected_supervisor_fault`] deliberately armed: present
/// (protection violation, not not-present), caused by a CPL3 (user-mode)
/// access, a read (not a write), and not an instruction fetch - exactly
/// what a CPL3 data read of a mapped-but-supervisor-only page produces.
/// Disarms itself on a match so it cannot be reused for a later, unrelated
/// fault.
#[cfg(all(feature = "qemu-test", feature = "userspace-supervisor-fault-test"))]
fn take_expected_supervisor_fault_if_matches(addr: u64, error_code: PageFaultErrorCode) -> bool {
    if EXPECTED_SUPERVISOR_FAULT_ARMED.load(Ordering::Relaxed)
        && addr == EXPECTED_SUPERVISOR_FAULT_ADDRESS.load(Ordering::Relaxed)
        && error_code.contains(PageFaultErrorCode::PROTECTION_VIOLATION)
        && error_code.contains(PageFaultErrorCode::USER_MODE)
        && !error_code.contains(PageFaultErrorCode::CAUSED_BY_WRITE)
        && !error_code.contains(PageFaultErrorCode::INSTRUCTION_FETCH)
    {
        EXPECTED_SUPERVISOR_FAULT_ARMED.store(false, Ordering::Relaxed);
        true
    } else {
        false
    }
}

/// Deliberately reads from an unmapped virtual address to verify the
/// page-fault handler. Only invoked when the `fault-tests` feature is
/// enabled; never run during normal boot.
#[cfg(feature = "fault-tests")]
pub fn run_page_fault_test() {
    kprintln!("[test] WARNING: triggering an intentional page fault");

    // 0xdead_beef is not mapped in pvKernel's current address space, so
    // dereferencing it is expected to fault; that is the point of this test.
    let fault_address = 0xdead_beef as *const u8;

    #[cfg(feature = "qemu-test")]
    {
        EXPECTED_PAGE_FAULT.store(true, Ordering::Relaxed);
        EXPECTED_PAGE_FAULT_ADDRESS.store(fault_address as u64, Ordering::Relaxed);
    }

    // SAFETY: this read is not memory-safe; it is an intentional fault used
    // only to exercise the page-fault handler when `fault-tests` is enabled.
    // The handler never returns to this call site, so the invalid read never
    // produces a usable value.
    unsafe {
        core::ptr::read_volatile(fault_address);
    }
}

/// Fails the current QEMU test run, if any, and halts. Never returns.
fn fatal_exit() -> ! {
    #[cfg(feature = "qemu-test")]
    qemu::exit_failure();

    #[cfg(not(feature = "qemu-test"))]
    halt_forever()
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    kprintln!("[exception] breakpoint");
    kprintln!("{:#?}", stack_frame);
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
    kprintln!("[exception] invalid opcode");
    kprintln!("{:#?}", stack_frame);
    fatal_exit();
}

extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    let cpl = stack_frame.code_segment.rpl();

    #[cfg(all(feature = "qemu-test", feature = "userspace-privilege-fault-test"))]
    if take_expected_privilege_fault_if_matches(cpl) {
        kprintln!("[exception] general protection fault");
        kprintln!("origin: CPL{}", cpl as u8);
        kprintln!("error code: {:#x}", error_code);
        kprintln!("{:#?}", stack_frame);
        kprintln!("[test] expected CPL3 privilege fault matched");
        qemu::exit_success();
    }

    kprintln!("[exception] general protection fault");
    kprintln!("origin: CPL{}", cpl as u8);
    kprintln!("error code: {:#x}", error_code);
    kprintln!("{:#?}", stack_frame);
    fatal_exit();
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    let faulting_address = Cr2::read_raw();
    let cpl = stack_frame.code_segment.rpl();

    #[cfg(all(feature = "qemu-test", feature = "supervisor-write-protection-test"))]
    if take_expected_write_protection_fault_if_matches(faulting_address, cpl, error_code) {
        kprintln!("[exception] page fault");
        kprintln!("faulting address (CR2): {:#x}", faulting_address);
        kprintln!("origin: CPL{}", cpl as u8);
        kprintln!("error code: {:?}", error_code);
        kprintln!("{:#?}", stack_frame);
        kprintln!("[test] expected supervisor write-protection fault matched");
        qemu::exit_success();
    }

    #[cfg(feature = "qemu-test")]
    if memory::take_expected_unmap_fault_if_matches(faulting_address, error_code) {
        kprintln!("[exception] page fault");
        kprintln!("faulting address (CR2): {:#x}", faulting_address);
        kprintln!("origin: CPL{}", cpl as u8);
        kprintln!("error code: {:?}", error_code);
        kprintln!("{:#?}", stack_frame);
        kprintln!("[test] expected post-unmap page fault matched");
        qemu::exit_success();
    }

    #[cfg(feature = "qemu-test")]
    if EXPECTED_PAGE_FAULT.load(Ordering::Relaxed)
        && faulting_address == EXPECTED_PAGE_FAULT_ADDRESS.load(Ordering::Relaxed)
    {
        kprintln!("[exception] page fault");
        kprintln!("faulting address (CR2): {:#x}", faulting_address);
        kprintln!("origin: CPL{}", cpl as u8);
        kprintln!("error code: {:?}", error_code);
        kprintln!("{:#?}", stack_frame);
        kprintln!("[test] expected page fault matched");
        qemu::exit_success();
    }

    #[cfg(all(feature = "qemu-test", feature = "userspace-supervisor-fault-test"))]
    if take_expected_supervisor_fault_if_matches(faulting_address, error_code) {
        kprintln!("[exception] page fault");
        kprintln!("faulting address (CR2): {:#x}", faulting_address);
        kprintln!("origin: CPL{}", cpl as u8);
        kprintln!("error code: {:?}", error_code);
        kprintln!("{:#?}", stack_frame);
        kprintln!("[test] expected CPL3 supervisor-page fault matched");
        qemu::exit_success();
    }

    kprintln!("[exception] page fault");
    kprintln!("faulting address (CR2): {:#x}", faulting_address);
    kprintln!("origin: CPL{}", cpl as u8);
    kprintln!("error code: {:?}", error_code);
    kprintln!("{:#?}", stack_frame);
    fatal_exit();
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) -> ! {
    kprintln!("[exception] double fault");
    kprintln!("error code: {:#x}", error_code);
    kprintln!("{:#?}", stack_frame);
    fatal_exit();
}

/// IRQ0 / vector 32. Records the tick before sending EOI, so a tick is never
/// counted as delivered before the kernel has actually observed it. Must not
/// log, allocate, lock, or format text: any of those could stall or recurse
/// while interrupting arbitrary kernel state.
///
/// Under `userspace-syscall-test`, also records (in a single atomic store,
/// no lock, no formatting) whether this tick interrupted CPL3 execution -
/// this is the only instrumentation the userspace syscall test needs, and
/// it must stay this cheap so the fast path above is never affected.
#[cfg_attr(not(feature = "userspace-syscall-test"), allow(unused_variables))]
extern "x86-interrupt" fn timer_interrupt_handler(stack_frame: InterruptStackFrame) {
    super::pit::record_tick();

    #[cfg(feature = "userspace-syscall-test")]
    if stack_frame.code_segment.rpl() == x86_64::PrivilegeLevel::Ring3 {
        super::syscall::test::TIMER_INTERRUPTED_CPL3.store(true, Ordering::Relaxed);
    }

    pic::notify_end_of_interrupt(IrqLine::Timer);
}

/// IRQ7 / vector 39. A spurious IRQ7 is raised by the master PIC itself (for
/// example due to electrical noise) without a real device requesting
/// service; the in-service register distinguishes the two cases.
extern "x86-interrupt" fn master_spurious_interrupt_handler(_stack_frame: InterruptStackFrame) {
    if pic::master_in_service() & IrqLine::Lpt1.bit() == 0 {
        // Spurious: the PIC is not actually servicing this IRQ, so no EOI
        // is sent for it.
        return;
    }

    pic::record_unexpected(IrqLine::Lpt1);
    pic::notify_end_of_interrupt(IrqLine::Lpt1);
}

/// IRQ15 / vector 47. Same reasoning as [`master_spurious_interrupt_handler`],
/// but on the slave PIC; the master's cascade line (IRQ2) still needs an EOI
/// either way, since the master did forward the cascaded signal.
extern "x86-interrupt" fn slave_spurious_interrupt_handler(_stack_frame: InterruptStackFrame) {
    if pic::slave_in_service() & IrqLine::SecondaryAta.bit() == 0 {
        pic::notify_end_of_interrupt(IrqLine::Cascade);
        return;
    }

    pic::record_unexpected(IrqLine::SecondaryAta);
    pic::notify_end_of_interrupt(IrqLine::SecondaryAta);
}

/// Generates a fixed handler for an IRQ line that pvKernel keeps masked in
/// 0.0.4 but must still install a valid IDT entry for. Any real interrupt
/// reaching one of these (which should not happen, since the line stays
/// masked) is recorded as unexpected and acknowledged.
macro_rules! generic_irq_handler {
    ($name:ident, $irq:expr) => {
        extern "x86-interrupt" fn $name(_stack_frame: InterruptStackFrame) {
            pic::record_unexpected($irq);
            pic::notify_end_of_interrupt($irq);
        }
    };
}

generic_irq_handler!(irq1_handler, IrqLine::Keyboard);
generic_irq_handler!(irq2_handler, IrqLine::Cascade);
generic_irq_handler!(irq3_handler, IrqLine::Com2);
generic_irq_handler!(irq4_handler, IrqLine::Com1);
generic_irq_handler!(irq5_handler, IrqLine::Lpt2);
generic_irq_handler!(irq6_handler, IrqLine::Floppy);
generic_irq_handler!(irq8_handler, IrqLine::RealTimeClock);
generic_irq_handler!(irq9_handler, IrqLine::Free9);
generic_irq_handler!(irq10_handler, IrqLine::Free10);
generic_irq_handler!(irq11_handler, IrqLine::Free11);
generic_irq_handler!(irq12_handler, IrqLine::Mouse);
generic_irq_handler!(irq13_handler, IrqLine::FpuCoprocessor);
generic_irq_handler!(irq14_handler, IrqLine::PrimaryAta);
