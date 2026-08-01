#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

mod arch;
mod drivers;
mod graphics;
mod halt;

use bootloader_api::{BootInfo, entry_point};
use core::panic::PanicInfo;

use arch::x86_64::pic::{self, IrqLine};
use arch::x86_64::pit;
#[cfg(not(feature = "qemu-test"))]
use halt::{halt_forever, idle_loop};

entry_point!(kernel_main);

/// Minimum number of timer ticks the boot-time test waits for; five repeated
/// ticks prove that the PIT keeps raising IRQ0, the PIC keeps delivering
/// vector 32, the handler keeps returning, and EOI keeps re-arming the PIC,
/// not just that a single interrupt happened to arrive.
const TIMER_TEST_TICKS: u64 = 5;

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    kprintln!();
    kprintln!("========================================");
    kprintln!("pvKernel 0.0.4");
    kprintln!("Architecture: x86_64");
    kprintln!("Boot mode: UEFI");
    kprintln!("========================================");

    let usable_memory: u64 = boot_info
        .memory_regions
        .iter()
        .filter(|region| region.kind == bootloader_api::info::MemoryRegionKind::Usable)
        .map(|region| region.end.saturating_sub(region.start))
        .sum();

    kprintln!(
        "Usable physical memory: {} MiB",
        usable_memory / 1024 / 1024
    );

    arch::init();

    if let Some(framebuffer) = boot_info.framebuffer.as_mut() {
        let info = framebuffer.info();

        kprintln!(
            "Framebuffer: {}x{}, stride {}, {} B/px, {:?}",
            info.width,
            info.height,
            info.stride,
            info.bytes_per_pixel,
            info.pixel_format
        );

        graphics::framebuffer::render_boot_screen(framebuffer);

        kprintln!("Framebuffer initialized");
    } else {
        kprintln!("WARNING: No framebuffer available");
    }

    arch::x86_64::interrupts::run_breakpoint_test();

    let ticks_before = pit::ticks();
    kprintln!("[test] waiting for timer interrupts");

    pic::unmask(IrqLine::Timer);
    kprintln!("[irq] PIT IRQ0 unmasked");

    x86_64::instructions::interrupts::enable();
    kprintln!("[cpu] maskable hardware interrupts enabled");

    pit::wait_for_ticks(TIMER_TEST_TICKS);

    let ticks_after = pit::ticks();
    kprintln!(
        "[test] timer ticks advanced from {} to {} (uptime ~{} ms)",
        ticks_before,
        ticks_after,
        pit::uptime_millis()
    );
    kprintln!("[test] timer tick counter advanced successfully");

    let unexpected_irqs = pic::take_unexpected_irq_mask();
    if unexpected_irqs != 0 {
        kprintln!(
            "[irq] WARNING: unexpected IRQ mask: {:#06b}",
            unexpected_irqs
        );
    }

    #[cfg(feature = "fault-tests")]
    arch::x86_64::interrupts::run_page_fault_test();

    kprintln!("pvKernel 0.0.4 initialized");
    kprintln!("entering interrupt-enabled idle loop");

    #[cfg(feature = "qemu-test")]
    arch::x86_64::qemu::exit_success();

    #[cfg(not(feature = "qemu-test"))]
    idle_loop();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kprintln!();
    kprintln!("========================================");
    kprintln!("pvKernel PANIC");
    kprintln!("{info}");
    kprintln!("========================================");

    #[cfg(feature = "qemu-test")]
    arch::x86_64::qemu::exit_failure();

    #[cfg(not(feature = "qemu-test"))]
    halt_forever()
}
