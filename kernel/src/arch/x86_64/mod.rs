pub mod gdt;
pub mod interrupts;
pub mod memory;
pub mod pic;
pub mod pit;
#[cfg(feature = "qemu-test")]
pub mod qemu;

use bootloader_api::BootInfo;

/// Target frequency for the PIT periodic timer; see [`pit::initialize`] for
/// how this maps to an actual divisor and configured frequency.
const PIT_TARGET_HZ: u32 = 100;

/// Brings the CPU into the state pvKernel expects before any other subsystem
/// runs: hardware interrupts masked, then a kernel-owned GDT/TSS, then a
/// kernel-owned IDT, then the physical frame allocator and active page
/// table, then the PIC remapped and fully masked, then the PIT configured.
/// This order matters: the IDT's double-fault entry relies on the IST stack
/// set up while installing the GDT/TSS; memory initialization must run
/// after the IDT so a page fault during it is still handled, and before the
/// PIC/PIT so no other subsystem can create page-table mappings first; the
/// PIC must not be touched before the IDT has valid entries for its
/// remapped vectors; and the PIT is configured while IRQ0 is still masked,
/// so no timer interrupt can arrive before the kernel is ready to handle
/// one. Maskable hardware interrupts stay disabled and IRQ0 stays masked
/// until the caller explicitly unmasks it and re-enables interrupts later
/// in boot.
pub fn init(boot_info: &mut BootInfo) {
    x86_64::instructions::interrupts::disable();
    crate::kprintln!("[cpu] maskable hardware interrupts disabled");

    gdt::init();
    interrupts::init();

    memory::init(boot_info);

    pic::initialize();
    pic::mask_all();
    crate::kprintln!("[irq] all hardware IRQs masked");

    pit::initialize(PIT_TARGET_HZ);
}
