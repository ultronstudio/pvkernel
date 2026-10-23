#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

mod arch;
mod drivers;
mod graphics;
mod halt;

use bootloader_api::config::{BootloaderConfig, Mapping};
use bootloader_api::{BootInfo, entry_point};
use core::panic::PanicInfo;

use arch::x86_64::pic::{self, IrqLine};
use arch::x86_64::pit;
#[cfg(not(feature = "qemu-test"))]
use halt::halt_forever;
#[cfg(all(not(feature = "qemu-test"), not(feature = "userspace-test")))]
use halt::idle_loop;

/// Requests a physical-memory offset mapping from the bootloader
/// (`BootInfo::physical_memory_offset`), which `arch::x86_64::memory` needs
/// to access the active page table (see [`arch::x86_64::memory::init`]).
/// The bootloader picks the actual virtual range dynamically at boot time;
/// pvKernel does not otherwise change the bootloader's default virtual
/// memory layout (kernel, stack, framebuffer, and boot-info mappings all
/// keep their default `Mapping::Dynamic` placement).
const fn bootloader_config() -> BootloaderConfig {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    config
}

static BOOTLOADER_CONFIG: BootloaderConfig = bootloader_config();

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

/// Minimum number of timer ticks the boot-time test waits for; five repeated
/// ticks prove that the PIT keeps raising IRQ0, the PIC keeps delivering
/// vector 32, the handler keeps returning, and EOI keeps re-arming the PIC,
/// not just that a single interrupt happened to arrive.
const TIMER_TEST_TICKS: u64 = 5;

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    kprintln!();
    kprintln!("========================================");
    kprintln!("pvKernel {}", env!("CARGO_PKG_VERSION"));
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

    // Takes ownership of the framebuffer out of `boot_info` *before*
    // `boot_info` itself is handed to `arch::init`, which consumes it
    // entirely (see that function and `arch::x86_64::memory::init` for why).
    // This is what lets `kernel_main` keep using the framebuffer afterward
    // without retaining any other access to `boot_info`.
    let mut framebuffer = boot_info.framebuffer.take();

    // `boot_info` is moved into `arch::init` here and not returned: after
    // this call, `kernel_main` has no further way to reach `BootInfo` (the
    // compiler enforces this, since `boot_info` is a `&'static mut`
    // reference and this function no longer has a live binding for it).
    arch::init(boot_info);

    arch::x86_64::memory::run_boot_smoke_test();

    if let Some(framebuffer) = framebuffer.as_mut() {
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

    #[cfg(feature = "mem-fault-tests")]
    arch::x86_64::memory::run_unmap_fault_test();

    // `run_test` never returns: every scenario it selects ends by exiting
    // QEMU or halting/panicking. Everything below is therefore compiled
    // out under `userspace-test`, rather than left as dead but reachable
    // code after a diverging call.
    #[cfg(feature = "userspace-test")]
    arch::x86_64::userspace::run_test();

    #[cfg(not(feature = "userspace-test"))]
    {
        kprintln!("pvKernel {} initialized", env!("CARGO_PKG_VERSION"));
        kprintln!("entering interrupt-enabled idle loop");

        #[cfg(feature = "qemu-test")]
        arch::x86_64::qemu::exit_success();

        #[cfg(not(feature = "qemu-test"))]
        idle_loop();
    }
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
