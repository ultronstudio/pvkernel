pub mod gdt;
pub mod interrupts;
pub mod memory;
pub mod pic;
pub mod pit;
#[cfg(feature = "qemu-test")]
pub mod qemu;
pub mod syscall;
pub mod task;
#[cfg(feature = "userspace-test")]
pub mod userspace;

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
///
/// Takes `boot_info` by value (not `&mut`) and passes it on into
/// [`memory::init`], which consumes it completely and does not return it.
/// This is deliberate: `kernel_main` extracts everything else it still
/// needs from `BootInfo` (currently just the framebuffer) *before* calling
/// this function, so that once `boot_info` is moved in here, nothing in the
/// kernel retains any other way to reach it. See [`memory::init`] for why
/// that matters.
pub fn init(boot_info: &'static mut BootInfo, framebuffer_mapping: Option<(u64, u64)>) {
    x86_64::instructions::interrupts::disable();
    crate::kprintln!("[cpu] maskable hardware interrupts disabled");

    gdt::init();
    interrupts::init();

    memory::init(boot_info, framebuffer_mapping);

    pic::initialize();
    pic::mask_all();
    crate::kprintln!("[irq] all hardware IRQs masked");

    pit::initialize(PIT_TARGET_HZ);
}
