//! Provisional bootstrap `int 0x80` syscall ABI.
//!
//! pvKernel 0.0.6 adds a **provisional, scalar-only** syscall ABI reachable
//! only via the software interrupt `int 0x80`: `SYSCALL`/`SYSRET` are not
//! implemented, there is no per-process or per-thread syscall entry
//! mechanism, and no syscall in this milestone accepts or dereferences a
//! user pointer. See
//! [docs/architecture/syscalls.md](../../../../../docs/architecture/syscalls.md)
//! for the full ABI description and rationale.
//!
//! # Layout
//!
//! - [`numbers`] - the syscall number constants (the `RAX` value on entry).
//! - [`SyscallError`] - the error representation encoded into a failing
//!   `RAX` result.
//! - [`frame::SyscallFrame`] - the register frame the entry stub builds and
//!   the dispatcher reads.
//! - `entry` - the naked-asm `int 0x80` entry stub (installed into the IDT
//!   by `arch::x86_64::interrupts::init`).
//! - `dispatch` - the Rust-side dispatcher the entry stub calls.
//! - `test` (only under the `userspace-test` Cargo feature) - test-only
//!   syscall numbers used solely by the deterministic userspace QEMU tests.

mod dispatch;
mod entry;
mod error;
mod frame;
pub mod numbers;
#[cfg(feature = "userspace-test")]
pub(crate) mod test;

pub use entry::pvkernel_syscall_entry_stub;
