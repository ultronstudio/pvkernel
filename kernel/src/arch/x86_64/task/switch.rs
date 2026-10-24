use core::arch::global_asm;

use super::Context;

// IRQ0 enters with hardware's five-word long-mode frame (RIP, CS, RFLAGS,
// RSP, SS) at the current RSP. The pushes below create the 15-word software
// prefix matching Context. Before Rust, we preserve that RSP in R12, align
// the temporary call stack down to 16 bytes, and clear DF for the ABI call.
// Rust returns a context RSP; the pops reverse the software prefix and IRETQ
// consumes the selected five-word hardware/synthetic frame. The IDT is an
// interrupt gate, so IF remains clear from entry until the selected RFLAGS
// image is restored.
global_asm!(
    ".global pvkernel_timer_irq_entry",
    "pvkernel_timer_irq_entry:",
    "push rax",
    "push rbx",
    "push rcx",
    "push rdx",
    "push rbp",
    "push rsi",
    "push rdi",
    "push r8",
    "push r9",
    "push r10",
    "push r11",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "mov r12, rsp",
    "mov rdi, rsp",
    "and rsp, -16",
    "cld",
    "call pvkernel_timer_schedule",
    "mov rsp, rax",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop r11",
    "pop r10",
    "pop r9",
    "pop r8",
    "pop rdi",
    "pop rsi",
    "pop rbp",
    "pop rdx",
    "pop rcx",
    "pop rbx",
    "pop rax",
    "iretq",
    ".global pvkernel_task_trampoline",
    "pvkernel_task_trampoline:",
    "and rsp, -16",
    "call rdi",
    "call pvkernel_task_exit",
    "2: hlt",
    "jmp 2b",
);

unsafe extern "C" {
    fn pvkernel_timer_irq_entry();
    fn pvkernel_task_trampoline();
}

pub fn timer_irq_entry() -> u64 {
    pvkernel_timer_irq_entry as *const () as u64
}
pub fn task_trampoline() -> u64 {
    pvkernel_task_trampoline as *const () as u64
}

#[unsafe(no_mangle)]
extern "C" fn pvkernel_timer_schedule(context: *mut Context) -> *mut Context {
    crate::task::timer_schedule(context)
}

#[unsafe(no_mangle)]
extern "C" fn pvkernel_task_exit() -> ! {
    crate::task::task_exit()
}
