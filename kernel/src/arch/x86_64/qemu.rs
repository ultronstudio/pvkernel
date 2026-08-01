//! QEMU `isa-debug-exit` support for deterministic headless kernel tests.
//!
//! Compiled only under the `qemu-test` feature. The device this writes to is
//! only present when the root runner's `--headless-test` mode adds
//! `-device isa-debug-exit,iobase=0xf4,iosize=0x04`; see
//! [docs/development/testing.md](../../../../docs/development/testing.md).

use x86_64::instructions::port::PortWriteOnly;

use crate::halt::halt_forever;

const EXIT_PORT: u16 = 0xF4;

/// Status values written to the isa-debug-exit port. QEMU derives its own
/// process exit status as `(value << 1) | 1`, so `Success` (0x10) becomes
/// host status 33 and `Failure` (0x11) becomes host status 35; the runner
/// only treats the former as success.
#[repr(u32)]
enum ExitCode {
    Success = 0x10,
    Failure = 0x11,
}

/// Reports test success to the runner and exits QEMU. Never returns; if QEMU
/// does not actually exit, falls back to [`halt_forever`].
pub fn exit_success() -> ! {
    exit(ExitCode::Success)
}

/// Reports test failure to the runner and exits QEMU. Never returns; if QEMU
/// does not actually exit, falls back to [`halt_forever`].
pub fn exit_failure() -> ! {
    exit(ExitCode::Failure)
}

fn exit(code: ExitCode) -> ! {
    // SAFETY: 0xF4 is QEMU's isa-debug-exit I/O port. It only exists because
    // the root runner's `--headless-test` mode adds the corresponding
    // `-device isa-debug-exit`; this module is compiled only under the
    // `qemu-test` feature reserved for those test builds, so the write is
    // never reached on real hardware or a plain `cargo run` boot.
    unsafe {
        PortWriteOnly::<u32>::new(EXIT_PORT).write(code as u32);
    }

    halt_forever();
}
