//! The `int 0x80` entry stub: the only piece of pvKernel that runs between
//! a CPL3 `int 0x80` and the Rust dispatcher.
//!
//! # Contract
//!
//! Reached only via the CPU's own interrupt-gate mechanism (installed by
//! `arch::x86_64::interrupts::init`), never called directly as a normal
//! Rust function. On entry, the CPU has already:
//!
//! - switched to CPL0 and loaded `SS:RSP` from `TSS.privilege_stack_table[0]`
//!   (the dedicated RSP0 stack from `arch::x86_64::gdt`), since `int 0x80`'s
//!   gate has no IST index and this is a CPL3-to-CPL0 transition;
//! - pushed the 5-qword hardware interrupt frame (`RIP`, `CS`, `RFLAGS`,
//!   `RSP`, `SS`) onto that new stack. `int 0x80` is a software interrupt
//!   with no error code, so no error-code qword is pushed.
//!
//! This stub then:
//!
//! 1. pushes all 15 general-purpose registers (`RAX` first, `R15` last),
//!    producing exactly the [`super::frame::SyscallFrame`] layout in
//!    memory (see that type's compile-time offset assertions);
//! 2. passes the frame's address (the current `RSP`) to
//!    [`super::dispatch::pvkernel_syscall_dispatch`] as its sole argument,
//!    per the SysV `extern "C"` calling convention;
//! 3. overwrites the saved `RAX` slot with the dispatcher's `u64` return
//!    value;
//! 4. pops all 15 registers back (`R15` first, `RAX` last - now holding the
//!    result instead of the original syscall number);
//! 5. executes `iretq`, which pops the hardware frame and returns to CPL3
//!    at the saved `RIP`, restoring the saved `RSP`/`SS`/`CS`/`RFLAGS`.
//!
//! Every general-purpose register except `RAX` is therefore preserved
//! across `int 0x80` by construction - not by a case-by-case save/restore
//! decision in the dispatcher, but because this stub always saves and
//! restores all 15 registers regardless of which syscall runs.
//!
//! # Stack alignment
//!
//! `TSS.privilege_stack_table[0]`'s top address is 16-byte aligned (see
//! `arch::x86_64::gdt::syscall_stack_top`), and the x86_64 architecture
//! itself aligns `RSP` to 16 bytes before pushing the hardware frame for
//! any privilege-level-changing interrupt, so the hardware frame's push
//! (40 bytes = 2.5 x 16) leaves `RSP % 16 == 8` when this stub starts. This
//! stub's own 15 pushes (120 bytes = 7.5 x 16) then bring `RSP % 16` back
//! to `0` at exactly the point of `call`, satisfying the SysV requirement
//! that `RSP % 16 == 0` immediately before a `call` instruction. No padding
//! push is needed.

use core::arch::naked_asm;

use super::dispatch::pvkernel_syscall_dispatch;

/// The `int 0x80` entry point. See the module-level documentation for the
/// full contract; this is installed directly via
/// `x86_64::structures::idt::Entry::set_handler_addr` rather than
/// `set_handler_fn`, since it does not use the `x86-interrupt` calling
/// convention.
#[unsafe(naked)]
pub extern "C" fn pvkernel_syscall_entry_stub() {
    naked_asm!(
        // Save all general-purpose registers except RSP (implicitly saved
        // as part of the hardware frame already on the stack) in the exact
        // order that produces `SyscallFrame`'s layout: RAX pushed first
        // (ends up at the highest offset among these 15 slots, 112),
        // R15 pushed last (ends up at offset 0, the current RSP).
        "push rax",
        "push rbx",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",
        "push rbp",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        // Pass the frame's address as the dispatcher's sole `extern "C"`
        // argument (RDI, per SysV).
        "mov rdi, rsp",
        "call {dispatch}",
        // Overwrite the saved RAX slot (offset 112, i.e. 14 * 8, the 15th
        // and last-pushed-before-the-call slot from the current RSP) with
        // the dispatcher's u64 result; see SyscallFrame's compile-time
        // offset assertion for this same constant.
        "mov [rsp + 14 * 8], rax",
        // Restore registers in the reverse order they were pushed.
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rbp",
        "pop rdi",
        "pop rsi",
        "pop rdx",
        "pop rcx",
        "pop rbx",
        "pop rax",
        // Return to CPL3: pops RIP, CS, RFLAGS, RSP, SS from the hardware
        // frame and switches back to the caller's stack and privilege
        // level.
        "iretq",
        dispatch = sym pvkernel_syscall_dispatch,
    );
}
