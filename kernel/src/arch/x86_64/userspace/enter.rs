//! The narrow CPL0-to-CPL3 transition: hand-builds an `iretq` frame and
//! jumps into ring 3. Never returns.

use x86_64::VirtAddr;

use crate::arch::x86_64::gdt;

/// Enters ring 3 (CPL3) via `iretq`, beginning execution at `entry_rip`
/// with initial stack pointer `entry_rsp`. Never returns.
///
/// # Safety
///
/// The caller must guarantee:
///
/// - `entry_rip` is a canonical address mapped present, user-accessible,
///   and executable;
/// - `entry_rsp` is a canonical address within a page mapped present,
///   user-accessible, and writable, with enough room below it for
///   whatever the target code does with the stack;
/// - `arch::x86_64::init` has already run, so the ring-3 GDT selectors and
///   the RSP0 transition stack (`gdt::init`) are installed;
/// - the caller accepts that this function never returns - its only
///   defined continuations are a fault handler or a syscall that itself
///   never returns (such as the test-only `TEST_EXIT`).
pub unsafe fn enter_user_mode(entry_rip: VirtAddr, entry_rsp: VirtAddr) -> ! {
    let new_cs = gdt::user_code_selector().0 as u64;
    let new_ss = gdt::user_data_selector().0 as u64;
    let new_rip = entry_rip.as_u64();
    let new_rsp = entry_rsp.as_u64();

    // Sanitized RFLAGS for the new CPL3 context: bit 1 is architecturally
    // fixed to 1; the interrupt flag is set so the PIT timer can still
    // interrupt CPL3 execution (`arch::x86_64::pit`); every other flag
    // (direction flag, IOPL, nested-task, virtual-8086, trap flag, ...) is
    // explicitly clear. This is a fixed constant, never derived from the
    // kernel's own current RFLAGS.
    const NEW_RFLAGS: u64 = 0x2 | (1 << 9);

    // SAFETY: forwarded from this function's own contract above. `iretq`
    // always pops the frame it acts on from the *current* stack,
    // regardless of whether the transition is hardware-initiated (raising
    // privilege) or, as here, software-initiated (lowering it) - the
    // frame's `RSP`/`SS` values are simply data telling the CPU which
    // (entirely separate) stack to switch to once it processes them. Each
    // general-purpose register is zeroed immediately after the five
    // pushes, so no kernel-only value (a pointer, a stack address, a
    // partially computed secret) leaks into the new CPL3 context; this is
    // safe even for whichever registers the compiler chose to hold the
    // five `in(reg)` values above, since those are only required to hold
    // their value long enough to be pushed, not to be preserved afterward.
    unsafe {
        core::arch::asm!(
            "push {new_ss}",
            "push {new_rsp}",
            "push {new_rflags}",
            "push {new_cs}",
            "push {new_rip}",
            "xor eax, eax",
            "xor ebx, ebx",
            "xor ecx, ecx",
            "xor edx, edx",
            "xor esi, esi",
            "xor edi, edi",
            "xor ebp, ebp",
            "xor r8, r8",
            "xor r9, r9",
            "xor r10, r10",
            "xor r11, r11",
            "xor r12, r12",
            "xor r13, r13",
            "xor r14, r14",
            "xor r15, r15",
            "iretq",
            new_ss = in(reg) new_ss,
            new_rsp = in(reg) new_rsp,
            new_rflags = in(reg) NEW_RFLAGS,
            new_cs = in(reg) new_cs,
            new_rip = in(reg) new_rip,
            options(noreturn),
        )
    }
}
