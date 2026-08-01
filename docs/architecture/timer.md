# Timer (PIT)

This document describes pvKernel's Intel 8253/8254-compatible Programmable
Interval Timer (PIT) driver, implemented in
[kernel/src/arch/x86_64/pit.rs](../../kernel/src/arch/x86_64/pit.rs), and how
it is wired into the boot flow and IDT.

## Why the legacy PIT, not HPET or the Local APIC timer

pvKernel 0.0.4 uses PIT channel 0 as its only timer source. It is available
on every x86_64 platform QEMU emulates, requires only a handful of I/O port
writes to configure, and its interrupt (IRQ0) is already reachable through
the [8259 PIC driver](hardware-interrupts.md) implemented in this same
milestone. HPET and the Local APIC timer are more capable (higher
resolution, per-CPU timers) but require additional discovery/MMIO
infrastructure that is out of scope here; they remain future roadmap items.

## Configuration

`pit::initialize(target_hz)` programs channel 0 for mode 2 (rate generator)
with lobyte/hibyte access (command byte `0x34`, command port `0x43`), then
writes a 16-bit divisor to the channel 0 data port (`0x40`), low byte first.
The divisor is derived from the PIT's fixed 1,193,182 Hz input clock:

```
divisor = round(1_193_182 / target_hz), clamped to 1..=65_536
actual_hz = 1_193_182 / divisor
```

Because the divisor must be an integer, `actual_hz` is generally not exactly
`target_hz`. `pit::initialize` returns the actual frequency and stores it in
`CONFIGURED_HZ`; callers (and `pit::uptime_millis`) use the actual value, not
the originally requested one. pvKernel requests 100 Hz at boot
(`arch::x86_64::mod::PIT_TARGET_HZ`); with the PIT's real input clock this
yields a divisor of 11,932 and an actual rate of approximately 99 Hz, logged
as `[timer] PIT configured at approximately 99 Hz`.

`pit::initialize` must run before PIT IRQ0 is unmasked, and does not itself
touch the PIC's mask register — masking/unmasking IRQ0 is the caller's
responsibility (see [Enabling the timer interrupt](#enabling-the-timer-interrupt)
below).

## Tick counter

`pit::TICKS` is an `AtomicU64` incremented by exactly one
(`Ordering::Relaxed`) each time `timer_interrupt_handler` (vector 32, IRQ0)
runs, via `pit::record_tick()`. `Relaxed` ordering is sufficient because the
counter is only ever read as a plain numeric observation (has at least N
ticks elapsed?); it is not used to publish or synchronize any other memory
state between the handler and its readers.

`pit::ticks()` reads the counter, and `pit::uptime_millis()` derives an
approximate uptime from the counter and the configured frequency (using a
`u128` intermediate product to avoid overflowing `u64` when multiplying a
large tick count by 1000).

## Waiting for ticks

`pit::wait_for_ticks(count)` spins on `hlt()`, comparing
`ticks().wrapping_sub(start)` against `count` on each wake, until at least
`count` further ticks have been recorded. It uses wrapping subtraction so it
remains correct even across an eventual wraparound of the 64-bit tick
counter. This must only be called after maskable hardware interrupts are
enabled and PIT IRQ0 is unmasked — otherwise no further tick ever arrives and
the call never returns.

## Enabling the timer interrupt

`kernel_main` performs a controlled, boot-time verification that timer
interrupts are actually being delivered, after the breakpoint test and
before the optional page-fault test:

1. record `ticks_before`;
2. `pic::unmask(IrqLine::Timer)` — enables IRQ0 specifically, leaving all
   other IRQ lines masked;
3. `x86_64::instructions::interrupts::enable()` — sets the CPU's maskable
   interrupt flag, the first point at which any hardware interrupt can
   actually reach the CPU in pvKernel's boot flow;
4. `pit::wait_for_ticks(TIMER_TEST_TICKS)` (5 ticks);
5. record `ticks_after` and log the observed tick advance and approximate
   uptime.

This ordering (unmask, then enable) matters: unmasking first with
interrupts still disabled cannot itself deliver an interrupt (the CPU's
interrupt flag gates delivery), so there is no window where an IRQ0 could
arrive before the handler and IDT are fully ready.

## Interrupt-enabled idle

After the boot-time checks complete, `kernel_main` enters
`halt::idle_loop`, which simply calls `hlt()` in a loop: each `hlt` parks the
CPU until the next interrupt (in practice, the next PIT tick) wakes it, the
timer handler runs and returns, and the loop parks again. This is
distinct from `halt::halt_forever`, used for fatal conditions, which
disables interrupts first and never expects to be woken. See
[docs/architecture/interrupts.md](interrupts.md#fatal-exception-behavior)
for the fatal path.
