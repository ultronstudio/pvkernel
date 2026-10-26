use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=userspace/init/Cargo.toml");
    println!("cargo:rerun-if-changed=userspace/init/build.rs");
    println!("cargo:rerun-if-changed=userspace/init/linker.ld");
    println!("cargo:rerun-if-changed=userspace/init/src");
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo does not set OUT_DIR"));

    let kernel_binary = PathBuf::from(
        std::env::var_os("CARGO_BIN_FILE_KERNEL_kernel")
            .expect("Cargo does not set CARGO_BIN_FILE_KERNEL_kernel path"),
    );

    let staged_kernel = out_dir.join("pvkernel-kernel.elf");
    std::fs::copy(&kernel_binary, &staged_kernel).expect("failed to stage kernel ELF");

    let init_binary = PathBuf::from(
        std::env::var_os("CARGO_BIN_FILE_PVKERNEL_INIT_init")
            .expect("Cargo does not set CARGO_BIN_FILE_PVKERNEL_INIT_init path"),
    );
    let staged_init = out_dir.join("pvkernel-init.elf");
    std::fs::copy(&init_binary, &staged_init).expect("failed to stage userspace init ELF");
    let initrd_path = out_dir.join("pvkernel-initrd.cpio");
    let mut init_bytes = std::fs::read(&staged_init).expect("failed to read init ELF");
    let initrd = if env::var_os("CARGO_FEATURE_INITRD_CORRUPT_TEST").is_some() {
        b"not a CPIO archive".to_vec()
    } else {
        if env::var_os("CARGO_FEATURE_INITRD_INVALID_INIT_TEST").is_some() {
            init_bytes = b"not an ELF image".to_vec();
        }
        if env::var_os("CARGO_FEATURE_INITRD_WX_INIT_TEST").is_some() {
            mark_init_executable_writable(&mut init_bytes);
        }
        build_newc(
            &init_bytes,
            env::var_os("CARGO_FEATURE_INITRD_MISSING_INIT_TEST").is_none(),
        )
    };
    std::fs::write(&initrd_path, initrd).expect("failed to write deterministic initrd");

    let uefi_image = out_dir.join("pvkernel-uefi.img");

    let mut image = bootloader::UefiBoot::new(&kernel_binary);
    if env::var_os("CARGO_FEATURE_INITRD_MISSING_TEST").is_none() {
        image.set_ramdisk(&initrd_path);
    }
    image
        .create_disk_image(&uefi_image)
        .expect("UEFI image creation failed");

    println!("cargo:rustc-env=PVKERNEL_INITRD={}", initrd_path.display());

    println!(
        "cargo:rustc-env=PVKERNEL_UEFI_IMAGE={}",
        uefi_image.display()
    );
}

fn build_newc(init: &[u8], include_init: bool) -> Vec<u8> {
    let mut archive = Vec::new();
    append_entry(&mut archive, 1, b"pvOS", 0o040755, &[]);
    append_entry(&mut archive, 2, b"pvOS/System", 0o040755, &[]);
    if include_init {
        append_entry(&mut archive, 3, b"pvOS/System/Init", 0o100755, init);
    }
    append_entry(&mut archive, 4, b"TRAILER!!!", 0, &[]);
    archive
}

fn mark_init_executable_writable(bytes: &mut [u8]) {
    if bytes.len() < 64 || &bytes[..4] != b"\x7fELF" {
        panic!("cannot construct W^X fixture from invalid init ELF");
    }
    let phoff = usize::try_from(u64::from_le_bytes(bytes[32..40].try_into().unwrap()))
        .expect("init program header offset does not fit usize");
    let phentsize = usize::from(u16::from_le_bytes(bytes[54..56].try_into().unwrap()));
    let phnum = usize::from(u16::from_le_bytes(bytes[56..58].try_into().unwrap()));
    for index in 0..phnum {
        let at = phoff
            .checked_add(index.checked_mul(phentsize).expect("phdr offset overflow"))
            .expect("phdr range overflow");
        let end = at.checked_add(phentsize).expect("phdr end overflow");
        let header = bytes
            .get_mut(at..end)
            .expect("init ELF has truncated program headers");
        if header.get(0..4) == Some(&1u32.to_le_bytes()) {
            let flags = u32::from_le_bytes(header[4..8].try_into().unwrap()) | 0b010;
            header[4..8].copy_from_slice(&flags.to_le_bytes());
        }
    }
}

fn append_entry(output: &mut Vec<u8>, inode: u32, name: &[u8], mode: u32, data: &[u8]) {
    output.extend_from_slice(b"070701");
    let fields = [
        inode,
        mode,
        0,
        0,
        if mode & 0o170000 == 0o040000 { 2 } else { 1 },
        0,
        u32::try_from(data.len()).expect("init ELF exceeds newc's 32-bit size field"),
        0,
        0,
        0,
        0,
        u32::try_from(name.len() + 1).expect("newc filename length overflow"),
        0,
    ];
    for field in fields {
        output.extend_from_slice(format!("{field:08x}").as_bytes());
    }
    output.extend_from_slice(name);
    output.push(0);
    while output.len() % 4 != 0 {
        output.push(0);
    }
    output.extend_from_slice(data);
    while output.len() % 4 != 0 {
        output.push(0);
    }
}
