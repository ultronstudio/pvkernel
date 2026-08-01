//! Rust-side `int 0x80` syscall dispatcher.
//!
//! [`pvkernel_syscall_dispatch`] is called only from `super::entry`'s
//! naked-asm stub, through the `extern "C"` boundary documented there. It
//! is never called directly from ordinary kernel code, and never called
//! with a frame that did not just come from a live `int 0x80`.

use x86_64::PrivilegeLevel;

use super::error::SyscallError;
use super::frame::SyscallFrame;
use super::numbers;
use crate::kprintln;

/// Dispatches one `int 0x80` syscall and returns its `RAX`-encoded result.
///
/// # Safety
///
/// `frame` must point to a valid, currently-live [`SyscallFrame`] on the
/// RSP0 stack, exactly as constructed by `super::entry`'s naked-asm stub
/// immediately before this call. This function only reads scalar integer
/// fields out of it and never retains the pointer past this call.
pub(super) extern "C" fn pvkernel_syscall_dispatch(frame: *const SyscallFrame) -> u64 {
    // SAFETY: forwarded from this function's own contract above, upheld by
    // its sole caller (`super::entry`'s naked-asm stub).
    let frame = unsafe { &*frame };

    // Confirms the entry stub actually built this frame on the dedicated
    // RSP0 ring-transition stack (see `gdt::syscall_stack_top`) rather than
    // some other stack - only meaningful under `userspace-test`, since only
    // its deterministic scenarios control the full call path closely enough
    // for this to be a useful invariant rather than noise.
    #[cfg(feature = "userspace-test")]
    {
        let (bottom, top) = super::super::gdt::rsp0_stack_range();
        let frame_addr = frame as *const SyscallFrame as u64;
        debug_assert!(
            frame_addr >= bottom && frame_addr < top,
            "syscall frame at {:#x} is outside the RSP0 stack range [{:#x}, {:#x})",
            frame_addr,
            bottom,
            top
        );
    }

    if frame.caller_privilege_level() != PrivilegeLevel::Ring3 {
        // Nothing in pvKernel 0.0.6 legitimately invokes `int 0x80` from
        // CPL0; this is a kernel bug, not an ordinary user-triggerable
        // error. It is still reported through the normal error-return path
        // (rather than panicking) so that a stray kernel-mode `int 0x80`
        // cannot itself crash the kernel - but it is loud on serial output
        // so the bug is not silently swallowed either.
        kprintln!(
            "[syscall] BUG: int 0x80 invoked from CPL{} (expected CPL3)",
            frame.cs & 0b11
        );
        return SyscallError::InvalidContext.encode();
    }

    // Every argument register is treated as an opaque integer until a
    // specific syscall validates it; no bootstrap syscall in 0.0.6
    // dereferences one as a pointer.
    let number = frame.rax;
    let arg0 = frame.rdi;

    match number {
        numbers::ABI_VERSION_QUERY => numbers::ABI_VERSION,
        numbers::DEBUG_WRITE_BYTE => debug_write_byte(arg0),
        numbers::TIMER_TICKS => crate::arch::x86_64::pit::ticks(),
        #[cfg(feature = "userspace-test")]
        n if n >= numbers::TEST_NAMESPACE_BASE => super::test::dispatch(n, frame),
        _ => SyscallError::Unsupported.encode(),
    }
}

/// Implements [`numbers::DEBUG_WRITE_BYTE`].
fn debug_write_byte(byte: u64) -> u64 {
    kprintln!("[syscall] debug byte from CPL3: {:#04x}", byte as u8);
    0
}
