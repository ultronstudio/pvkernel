#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

mod arch;
mod drivers;
mod graphics;
mod halt;

use bootloader_api::{BootInfo, entry_point};
use core::panic::PanicInfo;

use halt::halt_loop;

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    kprintln!();
    kprintln!("========================================");
    kprintln!("pvKernel 0.0.3");
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

    #[cfg(feature = "fault-tests")]
    arch::x86_64::interrupts::run_page_fault_test();

    kprintln!("pvKernel 0.0.3 initialized");
    kprintln!("CPU halted");

    halt_loop();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kprintln!();
    kprintln!("========================================");
    kprintln!("pvKernel PANIC");
    kprintln!("{info}");
    kprintln!("========================================");

    halt_loop()
}
