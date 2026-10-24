use std::env;

fn main() {
    let target = env::var("TARGET").expect("Cargo did not report the kernel target triple");
    assert_eq!(
        target, "x86_64-unknown-none",
        "the kernel task scheduler is validated only for x86_64-unknown-none"
    );
    let architecture = env::var("CARGO_CFG_TARGET_ARCH")
        .expect("Cargo did not report the kernel target architecture");
    assert_eq!(architecture, "x86_64", "kernel tasks require x86_64");

    let enabled = env::var("CARGO_CFG_TARGET_FEATURE")
        .expect("Cargo did not report enabled kernel target features");
    for forbidden in [
        "mmx", "sse", "sse2", "sse3", "ssse3", "sse4.1", "sse4.2", "avx", "avx2",
    ] {
        assert!(
            !enabled.split(',').any(|feature| feature == forbidden),
            "kernel task preemption requires {forbidden} to remain disabled"
        );
    }
    println!("cargo:rerun-if-env-changed=TARGET");
    println!("cargo:rerun-if-env-changed=CARGO_CFG_TARGET_FEATURE");
}
