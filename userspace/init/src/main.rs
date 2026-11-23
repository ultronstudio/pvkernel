#![no_std]
#![no_main]

use core::panic::PanicInfo;
use pvkernel_user::{Error, File};

core::arch::global_asm!(
    ".global _start",
    ".type _start,@function",
    "_start:",
    "and rsp, -16",
    "call {init_main}",
    "ud2",
    init_main = sym init_main,
);

const ABI_VERSION: u64 = 1;
const INIT_PATH: &str = "Boot:\\pvOS\\System\\Init";

fn init_main() -> ! {
    if pvkernel_user::abi_version() != Ok(ABI_VERSION) {
        fail();
    }
    let pid = pvkernel_user::process_id().unwrap_or_else(|_| fail());
    if pid != 1 {
        // PID 1 starts this image once as a child to exercise process
        // publication, exit handoff, wait status transfer, and reap.
        let _file = File::open(INIT_PATH).unwrap_or_else(|_| fail());
        pvkernel_user::exit(0);
    }

    let mut image = File::open(INIT_PATH).unwrap_or_else(|_| fail());
    let mut magic = [0; 4];
    let read = image.read(&mut magic).unwrap_or_else(|_| fail());
    if read != magic.len() || magic != *b"\x7fELF" {
        fail();
    }
    image.close().unwrap_or_else(|_| fail());

    if !matches!(File::open("Boot:\\missing"), Err(Error::NotFound))
        || !matches!(File::open("Boot:\\pvOS"), Err(Error::InvalidOperation))
    {
        fail();
    }
    let files = [
        File::open(INIT_PATH).unwrap_or_else(|_| fail()),
        File::open(INIT_PATH).unwrap_or_else(|_| fail()),
        File::open(INIT_PATH).unwrap_or_else(|_| fail()),
        File::open(INIT_PATH).unwrap_or_else(|_| fail()),
        File::open(INIT_PATH).unwrap_or_else(|_| fail()),
        File::open(INIT_PATH).unwrap_or_else(|_| fail()),
        File::open(INIT_PATH).unwrap_or_else(|_| fail()),
        File::open(INIT_PATH).unwrap_or_else(|_| fail()),
    ];
    if !matches!(File::open(INIT_PATH), Err(Error::NoResources)) {
        fail();
    }
    for file in files {
        file.close().unwrap_or_else(|_| fail());
    }

    let child = pvkernel_user::spawn(INIT_PATH).unwrap_or_else(|_| fail());
    if child.get() <= 1 {
        fail();
    }
    let mut status = i32::MIN;
    loop {
        match pvkernel_user::wait_child(child, &mut status) {
            Ok(false) => core::hint::spin_loop(),
            Ok(true) if status == 0 => break,
            _ => fail(),
        }
    }

    // The child intentionally exited with one descriptor open. Its exit
    // handoff must release all descriptors before PID 1 reuses the table.
    let file = File::open(INIT_PATH).unwrap_or_else(|_| fail());
    file.close().unwrap_or_else(|_| fail());

    write_console(b"pvOS: Boot VFS, child spawn, wait and reap OK\r\n");
    pvkernel_user::debug_write_byte(b'I').unwrap_or_else(|_| fail());
    loop {
        core::hint::spin_loop();
    }
}

fn write_console(bytes: &[u8]) {
    if pvkernel_user::console_write(bytes).ok() != Some(bytes.len()) {
        fail();
    }
}

fn fail() -> ! {
    let _ = pvkernel_user::console_write(b"pvOS: init failed\r\n");
    if pvkernel_user::process_id().is_ok() {
        pvkernel_user::exit(1);
    }
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail()
}
