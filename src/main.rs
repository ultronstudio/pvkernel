use ovmf_prebuilt::{Arch, FileType, Prebuilt, Source};
use std::env;
use std::process::{Command, ExitCode};

/// Status QEMU's `isa-debug-exit` device reports to the host process when
/// the kernel writes its "success" code (0x10). QEMU derives its own exit
/// status as `(value << 1) | 1`, so only this exact status counts as a
/// passed test; any other status (including the "failure" code 0x11, which
/// becomes 35) is treated as a failure.
const QEMU_TEST_SUCCESS_STATUS: i32 = 0x21;

fn main() -> ExitCode {
    let headless_test = env::args().any(|arg| arg == "--headless-test");

    let uefi_image = env!("PVKERNEL_UEFI_IMAGE");

    println!("Spouštím pvKernel v QEMU...");
    println!("UEFI obraz: {uefi_image}");
    if headless_test {
        println!("Režim: headless test (bez grafického výstupu)");
    }

    // Stáhne OVMF při prvním spuštění a následně používá místní cache.
    let ovmf = Prebuilt::fetch(Source::LATEST, "target/ovmf")
        .expect("Nepodařilo se stáhnout nebo načíst OVMF firmware");

    let ovmf_code = ovmf.get_file(Arch::X64, FileType::Code);
    let ovmf_vars = ovmf.get_file(Arch::X64, FileType::Vars);

    let mut command = Command::new("qemu-system-x86_64");
    command
        .env("GDK_BACKEND", "x11")
        .arg("-machine")
        .arg("q35")
        .arg("-m")
        .arg("512M")
        .arg("-cpu")
        .arg("max")
        .arg("-name")
        .arg("pvKernel 0.0.6")
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
        ));

    if headless_test {
        // The debug-exit device only makes sense for deterministic test
        // runs: it lets the kernel report pass/fail directly to QEMU's own
        // process exit status, instead of the graphical window staying open
        // forever in the normal interrupt-enabled idle loop.
        command
            .arg("-display")
            .arg("none")
            .arg("-device")
            .arg("isa-debug-exit,iobase=0xf4,iosize=0x04");
    } else {
        command
            .arg("-vga")
            .arg("std")
            .arg("-display")
            .arg("gtk,gl=off");
    }

    let status = command
        .status()
        .expect("Nepodařilo se spustit qemu-system-x86_64");

    if headless_test {
        match status.code() {
            Some(QEMU_TEST_SUCCESS_STATUS) => ExitCode::SUCCESS,
            Some(code) => {
                eprintln!("QEMU test selhal, neočekávaný stavový kód: {code}");
                ExitCode::FAILURE
            }
            None => {
                eprintln!("QEMU test selhal, proces skončil signálem: {status}");
                ExitCode::FAILURE
            }
        }
    } else if status.success() {
        ExitCode::SUCCESS
    } else {
        eprintln!("QEMU skončilo s kódem: {status}");
        ExitCode::FAILURE
    }
}
