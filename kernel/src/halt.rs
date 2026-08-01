use x86_64::instructions::hlt;

/// Parks the CPU in a low-power loop. Never returns.
pub fn halt_loop() -> ! {
    loop {
        hlt();
    }
}
