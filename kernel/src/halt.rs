use x86_64::instructions::hlt;

/// Disables maskable hardware interrupts and parks the CPU forever.
///
/// Used for unrecoverable conditions (panics, fatal CPU exceptions,
/// unexpected faults): once here, pvKernel deliberately stops receiving
/// further timer or other hardware interrupts instead of continuing to run
/// in a known-bad state.
pub fn halt_forever() -> ! {
    x86_64::instructions::interrupts::disable();
    loop {
        hlt();
    }
}

/// Parks the CPU between hardware interrupts. Never returns.
///
/// Unlike [`halt_forever`], this expects maskable hardware interrupts to
/// already be enabled: each `hlt` wakes for the next interrupt, the
/// corresponding handler runs and returns, and the loop parks again.
///
/// Unused under `qemu-test`, where a successful boot always exits QEMU
/// (`arch::x86_64::qemu::exit_success`) instead of idling forever.
#[cfg_attr(feature = "qemu-test", allow(dead_code))]
pub fn idle_loop() -> ! {
    loop {
        hlt();
    }
}
