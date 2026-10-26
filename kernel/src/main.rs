#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]

extern crate alloc;

mod arch;
mod drivers;
mod fs;
mod graphics;
mod halt;
mod process;
mod task;

use bootloader_api::config::{BootloaderConfig, Mapping};
use bootloader_api::{BootInfo, entry_point};
use core::alloc::Layout;
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
/// pvKernel retains dynamic bootloader mappings but constrains their virtual
/// placement to the audited upper-half kernel arena so the process lower
/// canonical half remains user-only.
const fn bootloader_config() -> BootloaderConfig {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    // Bootloader 0.11.17 chooses Mapping::Dynamic addresses from contiguous
    // free PML4 entries. Keep every retained boot mapping in indices 256..287,
    // wholly inside the kernel half and immediately below pvKernel's explicit
    // index-288 mappings (scratch, heap and scheduler stacks).
    config.mappings.dynamic_range_start = Some(arch::x86_64::memory::BOOTLOADER_DYNAMIC_START);
    config.mappings.dynamic_range_end = Some(arch::x86_64::memory::BOOTLOADER_DYNAMIC_END_SENTINEL);
    config
}

static BOOTLOADER_CONFIG: BootloaderConfig = bootloader_config();

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

/// Minimum number of timer ticks the boot-time test waits for; five repeated
/// ticks prove that the PIT keeps raising IRQ0, the PIC keeps delivering
/// vector 32, the handler keeps returning, and EOI keeps re-arming the PIC,
/// not just that a single interrupt happened to arrive.
const TIMER_TEST_TICKS: u64 = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)] // Failure variants are constructed only in selected QEMU scenario builds.
enum BootFailure {
    MissingRamdisk,
    InvalidRamdiskMetadata,
    CorruptArchive,
    BootVolumeRegistration,
    MissingBootVolume,
    MissingInit,
    InvalidInitElf,
    UnexpectedSuccess,
}

fn fail_boot(failure: BootFailure) -> ! {
    let expected = match failure {
        BootFailure::MissingRamdisk => cfg!(feature = "initrd-missing-test"),
        BootFailure::CorruptArchive => cfg!(feature = "initrd-corrupt-test"),
        BootFailure::BootVolumeRegistration => {
            cfg!(feature = "initrd-volume-registration-test")
        }
        BootFailure::MissingBootVolume => cfg!(feature = "initrd-missing-volume-test"),
        BootFailure::MissingInit => cfg!(feature = "initrd-missing-init-test"),
        BootFailure::InvalidInitElf => {
            cfg!(any(
                feature = "initrd-invalid-init-test",
                feature = "initrd-wx-init-test"
            ))
        }
        BootFailure::InvalidRamdiskMetadata | BootFailure::UnexpectedSuccess => false,
    };
    if expected {
        kprintln!("[initrd-test] expected controlled boot rejection: {failure:?}");
        #[cfg(feature = "qemu-test")]
        arch::x86_64::qemu::exit_success();
    }
    kprintln!("[boot] fatal initial-volume failure: {failure:?}");
    #[cfg(feature = "qemu-test")]
    arch::x86_64::qemu::exit_failure();
    #[cfg(not(feature = "qemu-test"))]
    halt::halt_forever()
}

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
    let ramdisk_metadata = (boot_info.ramdisk_addr.into_option(), boot_info.ramdisk_len);
    let framebuffer_mapping = framebuffer.as_mut().map(|buffer| {
        let bytes = buffer.buffer_mut();
        (bytes.as_mut_ptr() as u64, bytes.len() as u64)
    });

    // `boot_info` is moved into `arch::init` here and not returned: after
    // this call, `kernel_main` has no further way to reach `BootInfo` (the
    // compiler enforces this, since `boot_info` is a `&'static mut`
    // reference and this function no longer has a live binding for it).
    arch::init(boot_info, framebuffer_mapping);

    arch::x86_64::memory::run_boot_smoke_test();
    arch::x86_64::memory::run_heap_smoke_tests();
    task::init();
    process::init();
    kprintln!("[initrd] validating ramdisk metadata");
    let ramdisk =
        // SAFETY: the pinned bootloader mapping is retained and checked by
        // memory::init's boot-mapping audit before this immutable view exists.
        unsafe { fs::BootRamdisk::from_mapping(ramdisk_metadata.0, ramdisk_metadata.1) }
            .unwrap_or_else(|error| {
                let failure = if error == fs::RamdiskError::Missing {
                    BootFailure::MissingRamdisk
                } else {
                    kprintln!("[initrd] invalid ramdisk metadata: {error:?}");
                    BootFailure::InvalidRamdiskMetadata
                };
                fail_boot(failure)
            });
    kprintln!(
        "[initrd] ramdisk detected ({} bytes)",
        ramdisk.bytes().len()
    );
    kprintln!("[initrd] validating CPIO newc archive");
    let archive = fs::initrd::Archive::validate(ramdisk.bytes()).unwrap_or_else(|error| {
        kprintln!("[initrd] CPIO validation failed: {error:?}");
        fail_boot(BootFailure::CorruptArchive)
    });
    kprintln!(
        "[initrd] archive validated ({} entries)",
        archive.entry_count()
    );
    #[cfg(not(feature = "initrd-missing-volume-test"))]
    {
        kprintln!("[vfs] registering initrd filesystem as Boot:");
        fs::vfs::register_boot_volume(archive).unwrap_or_else(|error| {
            kprintln!("[vfs] Boot: registration failed: {error:?}");
            fail_boot(BootFailure::BootVolumeRegistration)
        });
        kprintln!("[vfs] boot volume ready");
    }
    #[cfg(feature = "initrd-volume-registration-test")]
    match fs::vfs::register_boot_volume(archive) {
        Err(fs::vfs::FsError::DuplicateVolume) => {
            kprintln!("[vfs-test] duplicate Boot: registration rejected");
        }
        Err(error) => {
            kprintln!("[vfs-test] unexpected Boot: registration error: {error:?}");
            arch::x86_64::qemu::exit_failure();
        }
        Ok(()) => fail_boot(BootFailure::UnexpectedSuccess),
    }
    if let Err(error) = fs::vfs::run_boot_checks() {
        kprintln!("[vfs] Boot: volume lookup failed: {error:?}");
        let failure = if error == fs::vfs::FsError::VolumeNotFound {
            BootFailure::MissingBootVolume
        } else {
            BootFailure::BootVolumeRegistration
        };
        fail_boot(failure);
    }
    #[cfg(any(
        not(any(feature = "qemu-test", feature = "userspace-test")),
        feature = "initrd-boot-test",
        feature = "initrd-missing-init-test",
        feature = "initrd-invalid-init-test",
        feature = "initrd-wx-init-test"
    ))]
    {
        const INIT_PATH: &str = "Boot:\\pvOS\\System\\Init";
        kprintln!("[init] resolving {INIT_PATH}");
        kprintln!("[init] loading boot init");
        let init = match fs::vfs::spawn_path(None, INIT_PATH) {
            Ok(init) => init,
            Err(fs::vfs::SpawnPathError::Filesystem(fs::vfs::FsError::NotFound)) => {
                fail_boot(BootFailure::MissingInit)
            }
            Err(fs::vfs::SpawnPathError::Filesystem(fs::vfs::FsError::VolumeNotFound)) => {
                fail_boot(BootFailure::MissingBootVolume)
            }
            Err(fs::vfs::SpawnPathError::Process(process::SpawnElfError::Elf(_))) => {
                fail_boot(BootFailure::InvalidInitElf)
            }
            Err(error) => {
                kprintln!("[init] boot executable load rejected: {error:?}");
                fail_boot(BootFailure::InvalidInitElf)
            }
        };
        if cfg!(any(
            feature = "initrd-missing-init-test",
            feature = "initrd-invalid-init-test",
            feature = "initrd-wx-init-test"
        )) {
            fail_boot(BootFailure::UnexpectedSuccess);
        }
        let snapshot = process::snapshot(init).expect("published PID 1 process has no snapshot");
        assert_eq!(
            snapshot.pid.get(),
            1,
            "first userspace process is not PID 1"
        );
        kprintln!("[process] init created with PID 1");
        kprintln!("[init] boot init runnable");
    }
    #[cfg(feature = "qemu-test")]
    arch::x86_64::memory::run_address_space_smoke_test();
    #[cfg(feature = "qemu-test")]
    arch::x86_64::memory::run_address_space_switch_smoke_test();
    #[cfg(feature = "qemu-test")]
    task::run_prepared_user_task_test();
    #[cfg(feature = "qemu-test")]
    arch::x86_64::memory::run_user_stack_smoke_test();
    #[cfg(feature = "user-copy-test")]
    arch::x86_64::memory::run_user_copy_test();
    #[cfg(feature = "qemu-test")]
    process::elf::run_loader_qemu_tests();

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

    let scheduler_smoke_tasks = task::prepare_smoke_test();
    #[cfg(feature = "process-capacity-test")]
    task::run_process_capacity_test();
    #[cfg(feature = "process-preemption-test")]
    task::prepare_process_preemption_test();
    #[cfg(feature = "process-smoke-test")]
    task::prepare_process_smoke_test();
    #[cfg(feature = "address-space-isolation-test")]
    task::prepare_address_space_isolation_test();

    let ticks_before = pit::ticks();
    kprintln!("[test] waiting for timer interrupts");

    pic::unmask(IrqLine::Timer);
    kprintln!("[irq] PIT IRQ0 unmasked");

    x86_64::instructions::interrupts::enable();
    kprintln!("[cpu] maskable hardware interrupts enabled");
    kprintln!("[scheduler] timer preemption enabled");

    pit::wait_for_ticks(TIMER_TEST_TICKS);
    task::run_smoke_test(scheduler_smoke_tasks.0, scheduler_smoke_tasks.1);
    #[cfg(feature = "user-fault-containment-test")]
    task::prepare_user_fault_containment_test();

    #[cfg(feature = "process-preemption-test")]
    task::wait_for_process_preemption_test();
    #[cfg(feature = "process-smoke-test")]
    task::wait_for_process_smoke_test();
    #[cfg(feature = "user-fault-containment-test")]
    task::wait_for_user_fault_containment_test();
    #[cfg(feature = "address-space-isolation-test")]
    task::wait_for_address_space_isolation_test();

    #[cfg(feature = "scheduler-preemption-test")]
    task::run_preemption_test();

    #[cfg(feature = "task-capacity-test")]
    task::run_capacity_test();

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

    #[cfg(feature = "supervisor-write-protection-test")]
    arch::x86_64::memory::run_supervisor_write_protection_test();

    // `run_test` never returns: every scenario it selects ends by exiting
    // QEMU or halting/panicking. Everything below is therefore compiled
    // out under `userspace-test`, rather than left as dead but reachable
    // code after a diverging call.
    #[cfg(feature = "userspace-test")]
    arch::x86_64::userspace::run_test();

    #[cfg(feature = "task-stack-guard-fault-test")]
    task::run_stack_guard_fault_test();

    #[cfg(all(
        not(feature = "userspace-test"),
        not(feature = "task-stack-guard-fault-test")
    ))]
    {
        kprintln!("pvKernel {} initialized", env!("CARGO_PKG_VERSION"));
        kprintln!("entering interrupt-enabled idle loop");

        #[cfg(feature = "qemu-test")]
        arch::x86_64::qemu::exit_success();

        #[cfg(not(feature = "qemu-test"))]
        idle_loop();
    }
}

#[alloc_error_handler]
fn alloc_error(layout: Layout) -> ! {
    kprintln!(
        "[heap] allocation failed: size={} align={}",
        layout.size(),
        layout.align()
    );
    #[cfg(feature = "qemu-test")]
    arch::x86_64::qemu::exit_failure();
    #[cfg(not(feature = "qemu-test"))]
    halt_forever()
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
