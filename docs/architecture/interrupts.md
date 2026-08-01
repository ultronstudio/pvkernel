# Interrupts and exceptions

This document describes pvKernel's Interrupt Descriptor Table (IDT) and
exception handling, implemented in
[kernel/src/arch/x86_64/interrupts.rs](../../kernel/src/arch/x86_64/interrupts.rs)
and
[kernel/src/arch/x86_64/gdt.rs](../../kernel/src/arch/x86_64/gdt.rs).

## What the IDT is

The Interrupt Descriptor Table is an x86_64 CPU structure that maps each of
the 256 possible interrupt/exception vectors to a handler function, a code
segment selector, and flags such as which stack to use. pvKernel builds its
own `InterruptDescriptorTable` (from the `x86_64` crate) in
`arch::x86_64::interrupts::init`, stored in a `spin::Once` cell so it has a
stable `'static` address for the remainder of kernel execution.

## Why handlers use `extern "x86-interrupt"`

CPU exceptions and interrupts do not follow the normal Rust/System V calling
convention: the CPU itself pushes a stack frame (and sometimes an error code)
before jumping to the handler, and the handler must return using `iretq`
rather than a normal `ret`. The `x86-interrupt` calling convention, exposed by
the `x86_64` crate's handler function types, generates the correct prologue,
epilogue, and stack frame handling for this. pvKernel enables it with
`#![feature(abi_x86_interrupt)]` in
[kernel/src/main.rs](../../kernel/src/main.rs), matching the nightly
toolchain already required for this project.

## CPU exceptions vs. hardware interrupts

CPU exceptions (breakpoint, page fault, double fault, and similar) are raised
synchronously by the CPU itself, either due to executing an instruction like
`int3`, or due to a fault condition (invalid opcode, bad memory access, and
so on). Hardware interrupts (from a timer, keyboard, disk controller, etc.)
are raised asynchronously by external devices, and are only delivered while
the CPU's maskable interrupt flag is enabled.

## Why 0.0.2 handles exceptions but not external IRQs

0.0.2's scope is limited to CPU exception handling. No PIC or APIC is
initialized, and maskable hardware interrupts remain disabled
(`x86_64::instructions::interrupts::disable()`, called in
`arch::x86_64::init`). Handling external IRQs safely requires an interrupt
controller and per-device handlers, which are out of scope for this release
and planned for 0.0.3 (see [ROADMAP.md](../../ROADMAP.md)).

## Installed handlers

| Vector | Handler | Behavior |
| --- | --- | --- |
| Breakpoint (`int3`) | `breakpoint_handler` | Logs the event and the stack frame, then returns normally. |
| Invalid opcode | `invalid_opcode_handler` | Logs the event and the stack frame, then halts. |
| General protection fault | `general_protection_fault_handler` | Logs the event, error code, and stack frame, then halts. |
| Page fault | `page_fault_handler` | Logs CR2 (faulting address), the page-fault error flags, and the stack frame, then halts. |
| Double fault | `double_fault_handler` | Runs on the dedicated IST stack; logs the event, error code, and stack frame, then halts. Return type is `!`. |

### Breakpoint behavior

The breakpoint handler is the only non-fatal handler. It is exercised once
during normal boot via a controlled `x86_64::instructions::interrupts::int3()`
call in `arch::x86_64::interrupts::run_breakpoint_test`, and execution is
expected to resume immediately afterward.

### Fatal exception behavior

All other installed handlers are treated as fatal: they print structured
diagnostics and then call `halt::halt_loop()` directly. They do not call
`panic!`, since a panic-based path would add unnecessary complexity for a
condition that already requires halting.

### Page-fault CR2 diagnostics

On a page fault, the CPU stores the faulting virtual address in the CR2
control register. `page_fault_handler` reads it with
`x86_64::registers::control::Cr2::read_raw()` and logs it alongside the
`PageFaultErrorCode` flags (present/absent, read/write, user/supervisor, and
so on) and the interrupt stack frame.

## Double-fault IST stack and triple-fault risk

A double fault can occur while handling another exception, including while
the CPU is trying to push an exception frame onto an already-corrupted or
overflowed stack. If the double-fault handler used the current stack in that
situation, pushing its own exception frame could fault again, which the CPU
cannot handle and results in a triple fault (usually a QEMU/hardware reset).

To avoid this, `gdt::init` allocates a dedicated 5 × 4 KiB stack and installs
its top address into the TSS's Interrupt Stack Table at
`gdt::DOUBLE_FAULT_IST_INDEX`. The IDT's `double_fault` entry is configured
with `set_stack_index(DOUBLE_FAULT_IST_INDEX)`, so the CPU switches to this
known-good stack before invoking `double_fault_handler`, regardless of the
state of the stack that faulted.

## Initialization order

The IDT is loaded only after the GDT and TSS are active
(`arch::x86_64::init` calls `gdt::init()` before `interrupts::init()`),
because the double-fault entry's IST index is only meaningful once the TSS
containing that IST entry has been loaded into the task register.
