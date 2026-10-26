#![no_std]
#![no_main]

use core::panic::PanicInfo;

core::arch::global_asm!(
    ".global _start",
    ".type _start,@function",
    "_start:",
    "and rsp, -16",
    "call {init_main}",
    "ud2",
    init_main = sym init_main,
);

fn init_main() -> ! {
    let pid = syscall(3, 0);
    if pid != 1 {
        let _ = syscall(1, u64::from(b'!'));
        let _ = syscall(4, 1);
        loop {
            core::hint::spin_loop();
        }
    }
    // Deterministic serial-visible success marker consumed by QEMU scenarios.
    let _ = syscall(1, u64::from(b'I'));
    loop {
        core::hint::spin_loop();
    }
}

fn syscall(number: u64, argument: u64) -> u64 {
    let result: u64;
    // SAFETY: this uses pvKernel's scalar-only int 0x80 bootstrap ABI. The
    // kernel saves/restores user GPR state and accepts only scalar arguments.
    unsafe {
        core::arch::asm!("int 0x80", inlateout("rax") number => result, in("rdi") argument,
            lateout("rcx") _, lateout("r11") _, options(nostack));
    }
    result
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
