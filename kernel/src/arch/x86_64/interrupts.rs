#[cfg(feature = "qemu-test")]
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use spin::Once;
use x86_64::registers::control::Cr2;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

use super::gdt::DOUBLE_FAULT_IST_INDEX;
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

        idt
    });

    idt.load();

    kprintln!("[cpu] IDT loaded");
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
    kprintln!("[exception] general protection fault");
    kprintln!("error code: {:#x}", error_code);
    kprintln!("{:#?}", stack_frame);
    fatal_exit();
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    let faulting_address = Cr2::read_raw();

    #[cfg(feature = "qemu-test")]
    if EXPECTED_PAGE_FAULT.load(Ordering::Relaxed)
        && faulting_address == EXPECTED_PAGE_FAULT_ADDRESS.load(Ordering::Relaxed)
    {
        kprintln!("[exception] page fault");
        kprintln!("faulting address (CR2): {:#x}", faulting_address);
        kprintln!("error code: {:?}", error_code);
        kprintln!("{:#?}", stack_frame);
        kprintln!("[test] expected page fault matched");
        qemu::exit_success();
    }

    kprintln!("[exception] page fault");
    kprintln!("faulting address (CR2): {:#x}", faulting_address);
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
extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    super::pit::record_tick();
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
