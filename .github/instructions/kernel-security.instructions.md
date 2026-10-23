---
applyTo: "kernel/src/**/*.rs,src/**/*.rs,build.rs"
---

# Kernel and runner security review checklist

Complements `.github/copilot-instructions.md` (disclosure rules and the
enforced paging invariants live there and apply here; do not restate
exploit details in public review comments).

## Unsafe Rust

- Raw pointer validity, alignment, provenance and non-null assumptions.
- Aliasing: no `&mut` alongside another live reference to the same data,
  including `static mut` and interrupt-reachable state.
- Lifetime extension (`'static` from `from_raw_parts` or transmute) and
  who owns bootloader-provided structures such as `BootInfo` and
  `MemoryRegions`.
- Interrupt concurrency: data touched by a handler and by normal code,
  and whether the atomic `Ordering` used is actually sufficient.
- Page-table mutation: TLB invalidation, partial-failure state, and
  frames leaked or reused after an error path.
- `asm!` / `naked_asm!` contracts: clobbers, stack alignment, flags
  (`options(...)`), register preservation, red zone, `iretq` frame layout.
- ABI assumptions at `extern "C"` and `x86-interrupt` boundaries.

## Paging

- A new mapping path must go through the permission profile that rejects
  writable+executable; never set `WRITABLE`/`NO_EXECUTE`/`USER_ACCESSIBLE`
  flags by hand to bypass it.
- pvKernel does not currently enable or verify SMEP/SMAP. Treat any
  kernel (CPL0) execution or dereference of a `USER_ACCESSIBLE` page as
  needing explicit justification, and do not assume hardware prevents it.
- Address and range arithmetic must use checked operations, and addresses
  must be canonical before use.

## Syscalls and userspace

In 0.0.6 no syscall dereferences a user pointer and there is no
`copy_from_user` (see `docs/architecture/syscalls.md`). A change that
introduces either must be reviewed for:

- unchecked user pointers, length overflow, canonical-address checks and
  range-end wraparound;
- TOCTOU between validation and use (double fetch);
- access-right propagation and privilege-origin validation (CPL taken
  from the saved `CS`, never from user-controlled registers);
- register or stack contents leaking back to CPL3;
- error codes that reveal kernel addresses or state.

## Interrupts and exceptions

- Error-code interpretation per vector, and CPL origin of the fault.
- Interrupt-enable state across locks and handlers (deadlock/reentrancy).
- Stack selection: `TSS.RSP0` for CPL3 entry, IST for double fault.
- Recursion or nested faults inside an exception handler.
- Expected-fault test paths (`EXPECTED_*` flags) accidentally matching a
  production fault, or compiled into default builds.
