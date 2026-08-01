//! Intel 8253/8254-compatible PIT channel 0, configured as a periodic timer.
//!
//! This is a temporary early x86_64 foundation; a modern timer source (HPET
//! or the Local APIC timer) is expected to replace it in a later milestone.
//! Like [`super::pic`], this module is stateless with respect to its I/O
//! ports and assumes a single CPU; no port access is synchronized.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use x86_64::instructions::hlt;
use x86_64::instructions::port::PortWriteOnly;

/// The PIT's fixed input clock frequency.
const PIT_INPUT_HZ: u32 = 1_193_182;

const CHANNEL0_DATA: u16 = 0x40;
const COMMAND_PORT: u16 = 0x43;

/// Channel 0, lobyte/hibyte access mode, mode 2 (rate generator), binary
/// counting.
const COMMAND_BYTE: u8 = 0x34;

/// The PIT's 16-bit counter register only supports these divisor values; a
/// divisor of 65536 is encoded on the wire as 0.
const MIN_DIVISOR: u32 = 1;
const MAX_DIVISOR: u32 = 65_536;

/// Monotonic count of timer interrupts delivered so far.
///
/// Incremented with `Ordering::Relaxed` from the timer interrupt handler.
/// `Relaxed` is sufficient because this counter is only ever observed as a
/// plain numeric value (has at least N ticks elapsed?); it does not
/// synchronize or publish any other memory state between the handler and its
/// readers.
static TICKS: AtomicU64 = AtomicU64::new(0);

/// The timer frequency actually configured by [`initialize`], which may
/// differ slightly from the requested frequency because the PIT divisor is
/// an integer.
static CONFIGURED_HZ: AtomicU32 = AtomicU32::new(0);

/// Configures PIT channel 0 as a periodic (mode 2) timer at approximately
/// `target_hz`, and returns the frequency actually achieved.
///
/// Must run before PIT IRQ0 is unmasked. `target_hz` must be in
/// `1..=PIT_INPUT_HZ`; pvKernel only ever calls this with a fixed, valid
/// constant, so invalid input is rejected with an assertion rather than a
/// `Result`.
pub fn initialize(target_hz: u32) -> u32 {
    assert!(target_hz > 0, "PIT target frequency must be non-zero");
    assert!(
        target_hz <= PIT_INPUT_HZ,
        "PIT target frequency exceeds the input clock"
    );

    let divisor = ((PIT_INPUT_HZ + target_hz / 2) / target_hz).clamp(MIN_DIVISOR, MAX_DIVISOR);
    let actual_hz = PIT_INPUT_HZ / divisor;
    let divisor_word = (divisor % 65_536) as u16;

    // SAFETY: writes below reprogram PIT channel 0 through its documented
    // command/data port protocol (command byte, then divisor low byte, then
    // high byte). The caller (`arch::x86_64::init`) runs this once during
    // early boot before any code depends on tick delivery, and pvKernel is
    // single-core, so no other code accesses these ports concurrently.
    unsafe {
        PortWriteOnly::<u8>::new(COMMAND_PORT).write(COMMAND_BYTE);
        let mut data = PortWriteOnly::<u8>::new(CHANNEL0_DATA);
        data.write((divisor_word & 0xFF) as u8);
        data.write((divisor_word >> 8) as u8);
    }

    CONFIGURED_HZ.store(actual_hz, Ordering::Relaxed);
    crate::kprintln!(
        "[timer] PIT configured at approximately {} Hz",
        frequency_hz()
    );

    actual_hz
}

/// Records one timer interrupt. Called only from the timer IRQ handler,
/// before it sends EOI.
pub fn record_tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

/// Returns the number of timer interrupts delivered so far.
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Returns the timer frequency configured by [`initialize`], in Hz.
pub fn frequency_hz() -> u32 {
    CONFIGURED_HZ.load(Ordering::Relaxed)
}

/// Returns an approximate uptime in milliseconds, derived from the tick
/// counter and the configured frequency.
///
/// The intermediate product is computed in `u128` to avoid overflowing `u64`
/// when multiplying a large tick count by 1000.
pub fn uptime_millis() -> u64 {
    let hz = u128::from(frequency_hz().max(1));
    let ticks = u128::from(ticks());
    ((ticks * 1000) / hz) as u64
}

/// Blocks (executing `hlt` between checks) until at least `count` further
/// timer ticks have been recorded.
///
/// Must only be called after maskable hardware interrupts are enabled and
/// PIT IRQ0 is unmasked; otherwise no further tick ever arrives and this
/// never returns. Uses wrapping subtraction so it remains correct across an
/// eventual wraparound of the tick counter.
pub fn wait_for_ticks(count: u64) {
    let start = ticks();
    while ticks().wrapping_sub(start) < count {
        hlt();
    }
}
