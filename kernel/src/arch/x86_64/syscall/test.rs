//! Test-only syscall implementations, compiled only under the
//! `userspace-test` Cargo feature.
//!
//! These syscall numbers ([`numbers::TEST_STATUS`], [`numbers::TEST_EXIT`])
//! are never part of the documented bootstrap ABI in `super::numbers`; see
//! docs/development/userspace-testing.md.

use core::sync::atomic::{AtomicBool, Ordering};

use super::error::SyscallError;
use super::frame::SyscallFrame;
use super::numbers;
use crate::kprintln;

/// Set from the timer interrupt handler
/// (`arch::x86_64::interrupts::timer_interrupt_handler`) the first time a
/// PIT tick is observed to interrupt CPL3 execution. Read (and encoded
/// into the result) by [`numbers::TEST_STATUS`].
///
/// Never cleared once set: a single observed interrupt is sufficient to
/// satisfy the userspace syscall test's expectation that CPL3 execution
/// can be interrupted and resumed.
pub static TIMER_INTERRUPTED_CPL3: AtomicBool = AtomicBool::new(false);

/// Dispatches a test-only syscall number (`>= numbers::TEST_NAMESPACE_BASE`).
/// Called only from `super::dispatch::pvkernel_syscall_dispatch`.
pub(super) fn dispatch(number: u64, frame: &SyscallFrame) -> u64 {
    match number {
        numbers::TEST_STATUS => {
            let mut status = 0u64;
            if TIMER_INTERRUPTED_CPL3.load(Ordering::Relaxed) {
                status |= numbers::TEST_STATUS_TIMER_INTERRUPTED_CPL3;
            }
            status
        }
        numbers::TEST_EXIT => test_exit(frame.rdi),
        _ => SyscallError::Unsupported.encode(),
    }
}

/// Implements [`numbers::TEST_EXIT`]. Never returns to the caller.
fn test_exit(code: u64) -> u64 {
    let success = code == 0;

    kprintln!(
        "[syscall] test_exit({}) from CPL3 userspace payload",
        if success { "success" } else { "failure" }
    );

    #[cfg(feature = "qemu-test")]
    {
        if success {
            crate::arch::x86_64::qemu::exit_success();
        } else {
            crate::arch::x86_64::qemu::exit_failure();
        }
    }

    #[cfg(not(feature = "qemu-test"))]
    {
        if success {
            crate::halt::halt_forever();
        } else {
            panic!("userspace test payload reported failure via test_exit");
        }
    }
}
