# Provisional bootstrap syscall ABI

This document describes pvKernel's provisional, scalar-only `int 0x80`
syscall ABI, implemented in
[kernel/src/arch/x86_64/syscall/](../../kernel/src/arch/x86_64/syscall/). See
[docs/architecture/userspace-entry.md](userspace-entry.md) for how CPL3 code
capable of issuing this instruction gets there in the first place.

## Status: provisional and experimental in 0.0.9

This ABI is **not stable** and not borrowed from `errno.h`/POSIX/Linux
numbering. It began as the 0.0.6 bootstrap boundary and is now extended with
the first process-aware calls in 0.0.9. It remains experimental; the first
meaningful compatibility checkpoint is 0.1.0 (see [ROADMAP.md](../../ROADMAP.md)).

## Entry mechanism: `int 0x80` only

`SYSCALL`/`SYSRET` are **not implemented**. The only way to invoke a
syscall is the software interrupt `int 0x80`. `arch::x86_64::interrupts::init`
installs `syscall::pvkernel_syscall_entry_stub` at IDT vector `0x80`, using
the raw `set_handler_addr` API (rather than `set_handler_fn`) because the
stub is a naked-asm function that builds its own `iretq`-based return
sequence instead of following the `x86-interrupt` calling convention. The
gate's descriptor privilege level is explicitly set to `Ring3`
(`set_privilege_level`), since the default gate DPL only permits invocation
from CPL0 — without this, a CPL3 `int 0x80` would itself raise `#GP`.

## Register convention

* `RAX` — the syscall number on entry; the result on return.
* `RDI`, `RSI`, `RDX`, `R10`, `R8`, `R9` — up to six scalar arguments, in
  that order. (`R10` takes the traditional fourth-argument slot instead of
  `RCX`, matching the Linux `x86-64` convention, since `RCX` is clobbered by
  `SYSCALL` on hardware that supports it — kept for forward compatibility
  even though this milestone does not implement `SYSCALL`.)
* Every other general-purpose register (`RBX`, `RBP`, `R12`-`R15`, plus
  `RCX`/`R11` beyond their argument roles) is preserved across the call:
  the entry stub saves and restores all 15 GPRs around the dispatch.

No syscall in this milestone accepts or dereferences a user pointer —
every argument is treated as an opaque scalar integer.

## Result encoding

A syscall's result is encoded entirely in `RAX`:

* bit 63 clear → success; the remaining 63 bits are the return value;
* bit 63 set (`ERROR_BIT`) → failure; the remaining bits are a
  [`SyscallError`](../../kernel/src/arch/x86_64/syscall/error.rs) variant:
  * `Unsupported` (1) — the syscall number is not recognized;
  * `InvalidArgument` (2) — reserved; no current syscall validates an
    argument strictly enough to return this;
  * `PermissionDenied` (3) — reserved; no current syscall is restricted
    beyond "callable from CPL3 at all";
  * `InvalidContext` (4) — the gate was entered from CPL0, which is always
    a kernel bug rather than a user-triggerable error (see below).

## Documented syscall numbers

Always compiled in (`syscall::numbers`):

| Number | Name | Behavior |
| --- | --- | --- |
| 0 | `ABI_VERSION_QUERY` | Returns the current bootstrap ABI version (`1`). |
| 1 | `DEBUG_WRITE_BYTE` | Logs the byte in `RDI` to serial output; returns `0`. |
| 2 | `TIMER_TICKS` | Returns the current PIT tick counter (`pit::ticks()`). |
| 3 | `GETPID` | Returns the current process PID; returns `InvalidContext` when no process-owned user Task is current. |
| 4 | `EXIT` | Terminates the current process with `RDI as i32`; never returns to userspace. |

Additionally, under the `userspace-test` Cargo feature, a small test-only
namespace (`>= TEST_NAMESPACE_BASE`, `0x1000`) exists solely for the
deterministic tests — `TEST_STATUS` (returns a bitset, currently just
"has a PIT tick been observed while at CPL3") and `TEST_EXIT` (diverges:
exits QEMU under `qemu-test`, or halts/panics otherwise, based on whether
`RDI` is zero). These are never part of the documented, stable ABI.

## The entry stub and `SyscallFrame`

`syscall::entry::pvkernel_syscall_entry_stub` is a `#[unsafe(naked)]`
function containing hand-written `naked_asm!`. On `int 0x80`, the CPU has
already switched to the TSS.RSP0 stack (see
[docs/architecture/userspace-entry.md](userspace-entry.md)) and pushed a
5-qword hardware frame (`RIP`, `CS`, `RFLAGS`, `RSP`, `SS` — no error code,
since `int 0x80` is a software interrupt). The stub then:

1. pushes all 15 general-purpose registers, in a fixed order, producing
   exactly the `SyscallFrame` layout (`#[repr(C)]`, with compile-time
   `offset_of!` assertions covering every field);
2. passes a pointer to that frame (`mov rdi, rsp`) to
   `syscall::dispatch::pvkernel_syscall_dispatch`, an ordinary `extern "C"`
   Rust function;
3. overwrites the saved `RAX` slot with the dispatcher's `u64` result;
4. pops all 15 registers back (undoing step 1, restoring every register
   except `RAX`, which now holds the result);
5. executes `iretq` to return to CPL3.

The combined push/call/pop sequence keeps `RSP` 16-byte aligned at the
`call` instruction (a 40-byte hardware frame + 120 bytes of GPR pushes =
160 bytes, a multiple of 16), satisfying the System V ABI the Rust
dispatcher expects — independently guaranteed twice over, since the CPU
itself also aligns `RSP` to 16 bytes before pushing on any
privilege-level-changing interrupt.

## Dispatch and the CPL0-origin check

`syscall::dispatch::pvkernel_syscall_dispatch` first checks
`frame.caller_privilege_level()` (derived from the saved `CS`'s RPL bits).
A CPL0 origin is treated as a kernel bug, logged loudly, and reported through
the normal error-return path (`SyscallError::InvalidContext`) rather than
panicking. For CPL3, `GETPID` checks that the scheduler has a current
process-owned user Task. `EXIT` validates that same context, records Process
and Task as Exiting, releases locks while IF is clear, then deliberately
executes an STI;HLT terminal handoff so PIT can switch away. It never writes a
result or returns to the saved user RIP. Zombie publication is deferred until
a later timer entry runs on another stack. Other calls match the documented
numbers above (and, under `userspace-test`, the test-only namespace), falling
back to `SyscallError::Unsupported`.

## What this ABI does not provide

* No `SYSCALL`/`SYSRET`.
* No user-pointer arguments yet; `copy_from_user`/`copy_to_user` are not implemented.
* No stable ABI guarantee; the 0.0.9 process-aware extension remains experimental.
* No signal delivery, no blocking syscalls, no I/O beyond the debug
  byte-logging syscall.
