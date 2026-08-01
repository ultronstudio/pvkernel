# Userspace entry (CPL0 → CPL3)

This document describes pvKernel 0.0.6's first CPL0 → CPL3 transition,
implemented in
[kernel/src/arch/x86_64/userspace/](../../kernel/src/arch/x86_64/userspace/)
and the ring-3 GDT/TSS additions in
[kernel/src/arch/x86_64/gdt.rs](../../kernel/src/arch/x86_64/gdt.rs). See
[docs/architecture/syscalls.md](syscalls.md) for the syscall ABI userspace
code can reach once it is running, and
[docs/development/userspace-testing.md](../development/userspace-testing.md)
for how to run the three deterministic tests this milestone adds.

## Scope

0.0.6 adds a deliberately minimal, test-only ring-3 entry path:

* ring-3 (CPL3) code and data GDT descriptors;
* a dedicated TSS.RSP0 ring-transition kernel stack;
* `enter_user_mode`, which builds an `iretq` frame by hand and transitions
  into CPL3;
* `Permissions::USER_DATA`/`USER_CODE` page permissions;
* three hand-written assembly test payloads, each exercised by its own
  deterministic headless QEMU test.

There is **no process model, no threads, no scheduler, no ELF loader, and
no `copy_from_user`/`copy_to_user`** in this milestone. Every user program
in 0.0.6 is a fixed, hand-written test payload mapped into a fixed virtual
address range at boot — there is no general "load and run a program"
mechanism yet. `SYSCALL`/`SYSRET` are also out of scope; the only syscall
entry mechanism is the software interrupt `int 0x80` (see
[docs/architecture/syscalls.md](syscalls.md)).

## Ring-3 GDT descriptors and TSS.RSP0

`arch::x86_64::gdt::init` appends a ring-3 code segment descriptor and a
ring-3 data segment descriptor to the GDT (via the `x86_64` crate's
`Descriptor::user_code_segment`/`user_data_segment`), reachable afterward
through `gdt::user_code_selector()`/`gdt::user_data_selector()`. Both
selectors are already constructed with RPL 3 (bits 0-1 of the selector).

`gdt::init` also installs a dedicated TSS.RSP0 stack: whenever a CPL3 → CPL0
transition happens (an interrupt, exception, or `int 0x80` while the CPU is
executing at CPL3), the CPU automatically switches to the stack pointer in
`TaskStateSegment.privilege_stack_table[0]` before pushing anything. Without
this, such a transition would use whatever `RSP` happened to be active,
which is unsound once CPL3 code (which the kernel does not trust) controls
it. The RSP0 stack's backing storage (`SyscallStack`, 4 × 4 KiB,
`#[repr(align(16))]`) is verified writable at boot the same way the
existing double-fault IST stack is (`verify_syscall_stack_writable`), and
its top is confirmed 16-byte aligned (`syscall_stack_top`,
`debug_assert_eq!`) — required by the System V ABI once Rust code (the
syscall dispatcher) runs on it.

## `enter_user_mode`

`userspace::enter::enter_user_mode(entry_rip, entry_rsp) -> !` is the only
way pvKernel constructs a CPL0 → CPL3 transition. It:

1. builds a 5-qword `iretq` frame on the current (kernel) stack, in the
   order `iretq` expects: `SS`, `RSP`, `RFLAGS`, `CS`, `RIP` (pushed in that
   order, so `RIP` ends up at the lowest address / current `RSP`);
2. uses a sanitized `RFLAGS` value (`0x2 | INTERRUPT_FLAG`, i.e. IOPL 0,
   the architecturally-fixed bit 1 set, and interrupts enabled) rather than
   whatever the kernel's own `RFLAGS` happened to be;
3. zeroes **every** general-purpose register (`RAX` through `R15`)
   immediately before `iretq`, so no kernel value — a pointer, a stack
   address, sensitive data — ever leaks into userspace through a register;
4. executes `iretq`, which is guaranteed to never return to this function
   (`options(noreturn)`).

Callers must uphold `entry_rip`/`entry_rsp` pointing at already-mapped,
correctly-permissioned user code/stack pages — `enter_user_mode` itself
does not map anything.

## Virtual address layout

The reserved userspace range (`memory::USERSPACE_RANGE_BASE`,
`0x0000_6000_0000_0000`) is laid out, for every test scenario, as:

| Address | Contents |
| --- | --- |
| `RANGE_BASE` (`CODE_ADDR`) | The user code page (`Permissions::USER_CODE`: read-only, executable, user-accessible). |
| `RANGE_BASE + 0x1000` (`GUARD_ADDR`) | Deliberately left **unmapped** — a code-page overrun faults instead of continuing into whatever is mapped next. |
| `RANGE_BASE + 0x2000` (`STACK_ADDR`) | The user stack page (`Permissions::USER_DATA`: writable, non-executable, user-accessible). |

## W^X-safe code population

A user code page is never simultaneously writable and executable. Each test
payload is populated in three steps:

1. map the code page as `Permissions::USER_DATA` (writable, non-executable);
2. copy the payload's bytes in (`core::ptr::copy_nonoverlapping`);
3. switch the page to `Permissions::USER_CODE` (read-only, executable) via
   `update_page_permissions`.

`Permissions::to_flags` already rejects any writable-and-executable
combination outright (pvKernel's existing W^X policy from 0.0.5); this
sequence never asks it to produce one.

## The three test payloads

Each payload is hand-written x86_64 assembly (`global_asm!`), not compiled
Rust — 0.0.6 has no ELF loader, so there is no other way to produce a user
program yet. Exactly one is compiled in, selected by exactly one of three
mutually exclusive Cargo features (enforced by `compile_error!` guards in
`userspace::mod`):

* **`userspace-syscall-test`** — confirms CPL3 execution, sets several
  sentinel register values, exercises the `int 0x80` ABI (an unknown
  syscall number, `ABI_VERSION_QUERY`, `DEBUG_WRITE_BYTE`), re-verifies the
  sentinels are unchanged, waits for a PIT tick to be observed while at
  CPL3, then exits via the test-only `TEST_EXIT` syscall.
* **`userspace-privilege-fault-test`** — executes `cli` at CPL3. Since
  `enter_user_mode` always sets IOPL 0, this is an IOPL-sensitive
  instruction the CPU rejects, raising `#GP(0)`.
* **`userspace-supervisor-fault-test`** — reads from a fixed address mapped
  supervisor-only (`Permissions::KERNEL_DATA`, not user-accessible),
  raising `#PF` with the `USER_MODE` and `PROTECTION_VIOLATION` bits set.

See [docs/development/userspace-testing.md](../development/userspace-testing.md)
for expected output and how each scenario is matched and exited
deterministically.

## What this milestone does not implement

* `SYSCALL`/`SYSRET` (only `int 0x80`).
* Any process, thread, or scheduler concept — there is exactly one,
  kernel-selected user program per boot, chosen at compile time.
* An ELF loader — every payload is hand-written assembly.
* `copy_from_user`/`copy_to_user`, or any dereferencing of a user-supplied
  pointer by kernel code.
* Returning from userspace back into the kernel's normal boot flow —
  every test scenario ends by exiting QEMU or halting/panicking.
