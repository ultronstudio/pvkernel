fn main() {
    // `x86_64-unknown-none` defaults to a position-independent executable on
    // this pinned nightly. pvKernel's checked loader intentionally accepts
    // only ET_EXEC, so this binary is linked at its fixed 0x0040_0000 base.
    println!("cargo:rustc-link-arg-bin=init=-no-pie");
    println!(
        "cargo:rustc-link-arg-bin=init=-T{}",
        std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("linker.ld")
            .display()
    );
    println!("cargo:rerun-if-changed=linker.ld");
}
