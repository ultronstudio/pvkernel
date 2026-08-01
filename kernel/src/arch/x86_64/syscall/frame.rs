//! The register frame `arch::x86_64::syscall::entry`'s naked-asm `int 0x80`
//! stub builds on the RSP0 stack and hands to the Rust dispatcher.
//!
//! Field order and byte offsets here are load-bearing: `super::entry`'s
//! assembly pushes registers in exactly the order that produces this
//! layout, writes the dispatcher's `u64` result into the `rax` field's
//! stack slot, and then pops registers in the reverse order before
//! `iretq`. The compile-time assertions at the bottom of this file exist so
//! that any accidental reordering or resizing fails the build instead of
//! silently corrupting the syscall return path; see `super::entry` for the
//! full push/pop sequence these offsets correspond to.

/// The full CPL3-to-CPL0 `int 0x80` register frame.
///
/// Layout, lowest to highest address: the 15 general-purpose registers the
/// entry stub saves (in push order - the *last* register pushed is the
/// *first* field, since the stack grows down), immediately followed by the
/// CPU's own hardware-pushed interrupt frame. `int 0x80` is a software
/// interrupt with no error code, so no error-code field is present (compare
/// [`x86_64::structures::idt::InterruptStackFrameValue`], which this
/// mirrors for its trailing five fields).
#[repr(C)]
#[derive(Debug)]
pub struct SyscallFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    /// The syscall number on entry (per the register ABI: `RAX` = number).
    /// The entry stub overwrites this stack slot with the dispatcher's
    /// `u64` result before restoring registers, so by the time `iretq`
    /// runs, this same slot instead holds the result that `RAX` is
    /// restored to.
    pub rax: u64,
    /// Hardware-pushed return `RIP` (the instruction after `int 0x80` in
    /// user code).
    pub rip: u64,
    /// Hardware-pushed return `CS`. Only bits `1:0` (the selector's RPL)
    /// are meaningful for CPL determination; see
    /// [`SyscallFrame::caller_privilege_level`].
    pub cs: u64,
    /// Hardware-pushed `RFLAGS` at the time of `int 0x80`.
    pub rflags: u64,
    /// Hardware-pushed return `RSP` (the user stack pointer).
    pub rsp: u64,
    /// Hardware-pushed return `SS`.
    pub ss: u64,
}

impl SyscallFrame {
    /// The privilege level `int 0x80` was executed from, derived from the
    /// saved `CS` selector's RPL field (bits `1:0`).
    ///
    /// This is how the dispatcher distinguishes an expected CPL3 syscall
    /// from a CPL0 invocation (a kernel bug in this milestone - see
    /// `super::dispatch`).
    pub fn caller_privilege_level(&self) -> x86_64::PrivilegeLevel {
        x86_64::PrivilegeLevel::from_u16((self.cs & 0b11) as u16)
    }
}

// Compile-time layout assertions. `super::entry`'s assembly hardcodes the
// byte offset of `rax` (to write the dispatcher's result back before
// popping registers) and relies on the hardware-pushed frame beginning
// immediately after the 15th general-purpose register. If a field is ever
// added, removed, or reordered, these fail to compile instead of silently
// corrupting the syscall return path.
const _: () = assert!(core::mem::size_of::<SyscallFrame>() == 160);
const _: () = assert!(core::mem::offset_of!(SyscallFrame, r15) == 0);
const _: () = assert!(core::mem::offset_of!(SyscallFrame, rbx) == 104);
const _: () = assert!(core::mem::offset_of!(SyscallFrame, rax) == 112);
const _: () = assert!(core::mem::offset_of!(SyscallFrame, rip) == 120);
const _: () = assert!(core::mem::offset_of!(SyscallFrame, cs) == 128);
const _: () = assert!(core::mem::offset_of!(SyscallFrame, rflags) == 136);
const _: () = assert!(core::mem::offset_of!(SyscallFrame, rsp) == 144);
const _: () = assert!(core::mem::offset_of!(SyscallFrame, ss) == 152);
