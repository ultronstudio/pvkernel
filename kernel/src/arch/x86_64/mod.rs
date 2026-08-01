pub mod gdt;
pub mod interrupts;

/// Brings the CPU into the state pvKernel expects before any other subsystem
/// runs: hardware interrupts masked, then a kernel-owned GDT/TSS, then a
/// kernel-owned IDT. This order matters: the IDT's double-fault entry relies
/// on the IST stack set up while installing the GDT/TSS.
pub fn init() {
    x86_64::instructions::interrupts::disable();
    crate::kprintln!("[cpu] maskable hardware interrupts disabled");

    gdt::init();
    interrupts::init();
}
