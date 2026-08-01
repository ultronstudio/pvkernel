use ovmf_prebuilt::{Arch, FileType, Prebuilt, Source};
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let uefi_image = env!("PVKERNEL_UEFI_IMAGE");

    println!("Spouštím pvKernel v QEMU...");
    println!("UEFI obraz: {uefi_image}");

    // Stáhne OVMF při prvním spuštění a následně používá místní cache.
    let ovmf = Prebuilt::fetch(Source::LATEST, "target/ovmf")
        .expect("Nepodařilo se stáhnout nebo načíst OVMF firmware");

    let ovmf_code = ovmf.get_file(Arch::X64, FileType::Code);
    let ovmf_vars = ovmf.get_file(Arch::X64, FileType::Vars);

    let status = Command::new("qemu-system-x86_64")
        .env("GDK_BACKEND", "x11")
        .arg("-machine")
        .arg("q35")
        .arg("-m")
        .arg("512M")
        .arg("-cpu")
        .arg("max")
        .arg("-vga")
        .arg("std")
        .arg("-display")
        .arg("gtk,gl=off")
        .arg("-name")
        .arg("pvKernel 0.0.2")
        .arg("-serial")
        .arg("stdio")
        .arg("-monitor")
        .arg("none")
        .arg("-no-reboot")
        .arg("-drive")
        .arg(format!("format=raw,file={uefi_image}"))
        .arg("-drive")
        .arg(format!(
            "if=pflash,format=raw,unit=0,file={},readonly=on",
            ovmf_code.display()
        ))
        .arg("-drive")
        .arg(format!(
            "if=pflash,format=raw,unit=1,file={},snapshot=on",
            ovmf_vars.display()
        ))
        .status()
        .expect("Nepodařilo se spustit qemu-system-x86_64");

    if status.success() {
        ExitCode::SUCCESS
    } else {
        eprintln!("QEMU skončilo s kódem: {status}");
        ExitCode::FAILURE
    }
}
