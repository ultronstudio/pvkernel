//! Hand-written assembly test payloads for the three deterministic
//! userspace QEMU tests.
//!
//! These are deliberately **not** compiled Rust: the entire point of these
//! tests is to exercise raw CPL3 execution, the `int 0x80` ABI, and fault
//! delivery without depending on the Rust compiler's own code generation
//! for the code actually running at CPL3. Each payload is self-contained
//! and position-independent - only relative branches, immediates, and
//! register operands; never an absolute reference to a kernel symbol or
//! address that could change across builds - and lives in a
//! supervisor-only `.rodata`-like section of the kernel image until
//! `super::map_test_program` copies it byte-for-byte into the mapped user
//! code page, before that page is switched to read-only + executable.
//!
//! Exactly one of these three `global_asm!` blocks (and its matching
//! accessor function) is compiled in for any given build, selected by
//! whichever single `userspace-*-test` Cargo feature is enabled.

use core::arch::global_asm;

#[cfg(feature = "userspace-syscall-test")]
global_asm!(
    ".pushsection .rodata.pvkernel_user_payload_syscall,\"a\"",
    ".align 16",
    ".global pvkernel_payload_syscall_test_start",
    "pvkernel_payload_syscall_test_start:",
    // 1. Verify this code is actually executing at CPL3 (CS RPL == 3).
    "mov ax, cs",
    "and al, 3",
    "cmp al, 3",
    "jne 2f",
    // 2. Set sentinel values in registers the `int 0x80` ABI documents as
    //    preserved across a syscall, to check later that they survive.
    "mov rbx, 0x1111111111111111",
    "mov rbp, 0x2222222222222222",
    "mov r12, 0x3333333333333333",
    "mov r13, 0x4444444444444444",
    "mov r14, 0x5555555555555555",
    "mov r15, 0x6666666666666666",
    // 3. An unknown syscall number must return
    //    SyscallError::Unsupported.encode() == (1 << 63) | 1.
    "mov rax, 0xdead",
    "int 0x80",
    "mov rcx, 0x8000000000000001",
    "cmp rax, rcx",
    "jne 2f",
    // 4. ABI_VERSION_QUERY (0) must return numbers::ABI_VERSION (1).
    "mov rax, 0",
    "int 0x80",
    "cmp rax, 1",
    "jne 2f",
    // 5. DEBUG_WRITE_BYTE (1) with an arbitrary byte must return 0.
    "mov rax, 1",
    "mov rdi, 0x42",
    "int 0x80",
    "cmp rax, 0",
    "jne 2f",
    // 6. Confirm every sentinel register from step 2 survived three
    //    syscalls unmodified.
    "mov rcx, 0x1111111111111111",
    "cmp rbx, rcx",
    "jne 2f",
    "mov rcx, 0x2222222222222222",
    "cmp rbp, rcx",
    "jne 2f",
    "mov rcx, 0x3333333333333333",
    "cmp r12, rcx",
    "jne 2f",
    "mov rcx, 0x4444444444444444",
    "cmp r13, rcx",
    "jne 2f",
    "mov rcx, 0x5555555555555555",
    "cmp r14, rcx",
    "jne 2f",
    "mov rcx, 0x6666666666666666",
    "cmp r15, rcx",
    "jne 2f",
    // 7. Poll TEST_STATUS (0x1000) for the timer-interrupted-CPL3 bit, up
    //    to a fixed iteration bound, so this can never hang forever if a
    //    timer interrupt never arrives.
    "xor r10, r10",
    "3:",
    "mov rax, 0x1000",
    "int 0x80",
    "test rax, 1",
    "jnz 4f",
    "inc r10",
    "cmp r10, 100000000",
    "jae 2f",
    "pause",
    "jmp 3b",
    // 8. Success: TEST_EXIT(0). Never returns; UD2 is a safety net in case
    //    it somehow did.
    "4:",
    "mov rax, 0x1001",
    "xor rdi, rdi",
    "int 0x80",
    "ud2",
    // Failure path: TEST_EXIT(1). Never returns; UD2 is a safety net.
    "2:",
    "mov rax, 0x1001",
    "mov rdi, 1",
    "int 0x80",
    "ud2",
    ".global pvkernel_payload_syscall_test_end",
    "pvkernel_payload_syscall_test_end:",
    ".popsection",
);

#[cfg(feature = "userspace-syscall-test")]
unsafe extern "C" {
    static pvkernel_payload_syscall_test_start: u8;
    static pvkernel_payload_syscall_test_end: u8;
}

/// Returns the syscall-boundary test payload's bytes, as a slice into the
/// kernel's own (supervisor-only) `.rodata`-like section.
#[cfg(feature = "userspace-syscall-test")]
pub(super) fn syscall_test_payload() -> &'static [u8] {
    // SAFETY: both symbols are defined by the `global_asm!` block above, in
    // the same translation unit, marking the start and end of one
    // contiguous byte range within a single section; only their addresses
    // are taken (the `u8` "value" behind each symbol is never read), and
    // the resulting slice only ever refers to this read-only kernel image
    // data, never to memory that could be mutated concurrently.
    unsafe {
        let start = &raw const pvkernel_payload_syscall_test_start as *const u8;
        let end = &raw const pvkernel_payload_syscall_test_end as *const u8;
        core::slice::from_raw_parts(start, end as usize - start as usize)
    }
}

#[cfg(feature = "userspace-privilege-fault-test")]
global_asm!(
    ".pushsection .rodata.pvkernel_user_payload_privilege_fault,\"a\"",
    ".align 16",
    ".global pvkernel_payload_privilege_fault_test_start",
    "pvkernel_payload_privilege_fault_test_start:",
    // CLI is IOPL-sensitive: executing it at CPL3 with IOPL 0 (as set by
    // `enter::enter_user_mode`'s sanitized RFLAGS) must raise #GP(0). UD2
    // is a safety net in case it somehow did not fault.
    "cli",
    "ud2",
    ".global pvkernel_payload_privilege_fault_test_end",
    "pvkernel_payload_privilege_fault_test_end:",
    ".popsection",
);

#[cfg(feature = "userspace-privilege-fault-test")]
unsafe extern "C" {
    static pvkernel_payload_privilege_fault_test_start: u8;
    static pvkernel_payload_privilege_fault_test_end: u8;
}

/// Returns the privilege-fault test payload's bytes. See
/// [`syscall_test_payload`] for the safety reasoning, which applies
/// identically here.
#[cfg(feature = "userspace-privilege-fault-test")]
pub(super) fn privilege_fault_payload() -> &'static [u8] {
    unsafe {
        let start = &raw const pvkernel_payload_privilege_fault_test_start as *const u8;
        let end = &raw const pvkernel_payload_privilege_fault_test_end as *const u8;
        core::slice::from_raw_parts(start, end as usize - start as usize)
    }
}

#[cfg(feature = "userspace-supervisor-fault-test")]
global_asm!(
    ".pushsection .rodata.pvkernel_user_payload_supervisor_fault,\"a\"",
    ".align 16",
    ".global pvkernel_payload_supervisor_fault_test_start",
    "pvkernel_payload_supervisor_fault_test_start:",
    // `super::SUPERVISOR_PROBE_ADDR` (memory::TEMP_MAPPING_BASE + 0x2000),
    // baked in directly as an immediate since this is internal,
    // kernel-controlled test code (not a user-supplied value); kept in
    // sync with the Rust-side constant via the compile-time assertion
    // below.
    "mov rax, 0xffff900000002000",
    "mov rbx, [rax]",
    "ud2",
    ".global pvkernel_payload_supervisor_fault_test_end",
    "pvkernel_payload_supervisor_fault_test_end:",
    ".popsection",
);

#[cfg(feature = "userspace-supervisor-fault-test")]
const _: () = assert!(super::SUPERVISOR_PROBE_ADDR == 0xFFFF_9000_0000_2000);

#[cfg(feature = "userspace-supervisor-fault-test")]
unsafe extern "C" {
    static pvkernel_payload_supervisor_fault_test_start: u8;
    static pvkernel_payload_supervisor_fault_test_end: u8;
}

/// Returns the supervisor-page-fault test payload's bytes. See
/// [`syscall_test_payload`] for the safety reasoning, which applies
/// identically here.
#[cfg(feature = "userspace-supervisor-fault-test")]
pub(super) fn supervisor_fault_payload() -> &'static [u8] {
    unsafe {
        let start = &raw const pvkernel_payload_supervisor_fault_test_start as *const u8;
        let end = &raw const pvkernel_payload_supervisor_fault_test_end as *const u8;
        core::slice::from_raw_parts(start, end as usize - start as usize)
    }
}
