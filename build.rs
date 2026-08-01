use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo does not set OUT_DIR"));

    let kernel_binary = PathBuf::from(
        std::env::var_os("CARGO_BIN_FILE_KERNEL_kernel")
            .expect("Cargo does not set CARGO_BIN_FILE_KERNEL_kernel path"),
    );

    let uefi_image = out_dir.join("pvkernel-uefi.img");

    bootloader::UefiBoot::new(&kernel_binary)
        .create_disk_image(&uefi_image)
        .expect("UEFI image creation failed");

    println!(
        "cargo:rustc-env=PVKERNEL_UEFI_IMAGE={}",
        uefi_image.display()
    );
}
