# Hardware interrupts (PIC)

This document describes pvKernel's legacy 8259 Programmable Interrupt
Controller (PIC) driver, implemented in
[kernel/src/arch/x86_64/pic.rs](../../kernel/src/arch/x86_64/pic.rs), and how
it feeds the IDT's IRQ vectors described in
[docs/architecture/interrupts.md](interrupts.md).

## Why the legacy PIC, not the APIC

x86 platforms expose two generations of interrupt controller: the legacy
dual 8259 PIC, and the newer Local/IO APIC. pvKernel 0.0.4 only implements
the 8259 PIC. It is simpler (two chained chips, a handful of I/O ports, no
MMIO or MSR configuration) and is sufficient to deliver a single periodic
timer interrupt, which is this milestone's goal. APIC support is not part of
this milestone and remains a future roadmap item.

## Vector remapping

The PIC's hardware IRQ lines (0-15) default to CPU vectors that overlap
pvKernel's CPU exception vectors (0-31), which would make a hardware IRQ
indistinguishable from a CPU exception. `pic::initialize` remaps them with
the standard Initialization Command Word (ICW) sequence:

- master PIC (command `0x20`, data `0x21`) → vectors 32-39
- slave PIC (command `0xA0`, data `0xA1`) → vectors 40-47
- the master is told a slave is cascaded on IRQ2 (ICW3 `0x04` on the master,
  `0x02` on the slave, its cascade identity)
- both chips are configured for 8086/88 mode (ICW4 `0x01`)

A short read from I/O port `0x80` (`io_wait`) is issued between each command
byte, since some hardware needs a small delay for the PIC to process each
ICW. All 16 IRQ lines are masked immediately after remapping
(`pic::mask_all`) and stay masked until a driver explicitly unmasks the one
it owns — see [docs/architecture/timer.md](timer.md) for the PIT/IRQ0 case,
the only line unmasked in 0.0.4.

## `IrqLine`

`pic::IrqLine` is a 16-variant enum (`Timer` through `SecondaryAta`,
matching the standard IRQ0-IRQ15 assignment) with helpers to compute:

- `vector()` — the remapped IDT vector (32 + IRQ number);
- `is_slave()` — whether the line is served by the slave PIC (IRQ8-15);
- `bit()` — the corresponding mask/ISR bit on whichever chip serves it.

[kernel/src/arch/x86_64/interrupts.rs](../../kernel/src/arch/x86_64/interrupts.rs)
uses `IrqLine::vector()` to install a handler for every remapped vector
(32-47), so no IRQ line is left with a missing IDT entry even though only
the timer is unmasked.

## Masking and unmasking

`pic::mask`/`pic::unmask` perform a read-modify-write on the owning chip's
data port (master `0x21` or slave `0xA1`), setting or clearing the bit
returned by `IrqLine::bit()`. Masking is per-line: unmasking IRQ0 (the
timer) does not affect any other line, all of which remain masked.

## End-of-interrupt (EOI) and the cascade line

Every interrupt handler must send an End-Of-Interrupt command (`0x20`) to
the PIC before returning, or the PIC will not deliver further interrupts on
that line (and, for the slave, on any slave line). `pic::notify_end_of_interrupt`
handles this correctly for both chips: if the IRQ is served by the slave, it
sends EOI to the slave first, then always sends EOI to the master (since the
slave is itself cascaded through the master's IRQ2 line).

## Spurious IRQ7/IRQ15 handling

The 8259 can raise IRQ7 (master) or IRQ15 (slave) spuriously — for example,
due to electrical noise on a masked line — without a real device requesting
service. A spurious IRQ must not be acknowledged with a normal EOI as if it
were real, because doing so can mask future legitimate interrupts on that
line.

pvKernel distinguishes real from spurious IRQ7/IRQ15 by reading the
In-Service Register (ISR) via OCW3 (`pic::master_in_service` /
`pic::slave_in_service`, command byte `0x0B` then a read of the same data
port): if the corresponding ISR bit is not set, the IRQ was spurious.

- **Spurious IRQ7** (master): no EOI is sent at all, since the master never
  actually latched it as in-service.
- **Spurious IRQ15** (slave): the slave is not in service, but the master
  *did* latch IRQ2 (the cascade line) as in-service, since it has no way to
  know the interrupt was spurious until the slave is queried. The handler
  sends EOI only to the master (reusing `notify_end_of_interrupt(IrqLine::
  Cascade)`, since that helper's master-only path for non-slave lines is
  exactly the correct spurious-IRQ15 behavior), and does not send EOI to the
  slave.

Both cases record the event in an internal `AtomicU16` unexpected-IRQ
bitmask (`pic::record_unexpected` / `pic::take_unexpected_irq_mask`), which
`kernel_main`'s boot-time timer verification checks and reports if any bit
is set.
