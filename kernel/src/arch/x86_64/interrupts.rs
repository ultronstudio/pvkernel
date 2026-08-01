use spin::Once;
use x86_64::registers::control::Cr2;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

use super::gdt::DOUBLE_FAULT_IST_INDEX;
use crate::halt::halt_loop;
use crate::kprintln;

static IDT: Once<InterruptDescriptorTable> = Once::new();

/// Installs the pvKernel IDT and loads it onto the current CPU.
///
/// Must run after [`super::gdt::init`], because the double-fault entry below
/// is only safe once the corresponding IST stack is active in the TSS.
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

/// Deliberately reads from an unmapped virtual address to verify the
/// page-fault handler. Only invoked when the `fault-tests` feature is
/// enabled; never run during normal boot.
#[cfg(feature = "fault-tests")]
pub fn run_page_fault_test() {
    kprintln!("[test] WARNING: triggering an intentional page fault");

    // 0xdead_beef is not mapped in pvKernel's current address space, so
    // dereferencing it is expected to fault; that is the point of this test.
    let fault_address = 0xdead_beef as *const u8;

    // SAFETY: this read is not memory-safe; it is an intentional fault used
    // only to exercise the page-fault handler when `fault-tests` is enabled.
    // The handler halts the CPU instead of returning, so the invalid read
    // never produces a usable value.
    unsafe {
        core::ptr::read_volatile(fault_address);
    }
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    kprintln!("[exception] breakpoint");
    kprintln!("{:#?}", stack_frame);
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
    kprintln!("[exception] invalid opcode");
    kprintln!("{:#?}", stack_frame);
    halt_loop();
}

extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    kprintln!("[exception] general protection fault");
    kprintln!("error code: {:#x}", error_code);
    kprintln!("{:#?}", stack_frame);
    halt_loop();
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    kprintln!("[exception] page fault");
    kprintln!("faulting address (CR2): {:#x}", Cr2::read_raw());
    kprintln!("error code: {:?}", error_code);
    kprintln!("{:#?}", stack_frame);
    halt_loop();
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) -> ! {
    kprintln!("[exception] double fault");
    kprintln!("error code: {:#x}", error_code);
    kprintln!("{:#?}", stack_frame);
    halt_loop();
}
