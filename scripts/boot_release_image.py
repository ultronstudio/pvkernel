#!/usr/bin/env python3
"""Smoke-test an actual production UEFI image, without rebuilding or test features.

Production Init remains alive instead of writing isa-debug-exit. Success
requires the ordered PID 1/CPL3 serial markers, followed by a short observation
window with QEMU still running and no fatal diagnostic. A timeout alone never
passes. Disk writes are redirected to QEMU's temporary snapshot.
"""

from __future__ import annotations

import argparse
import hashlib
from pathlib import Path
import subprocess
import tempfile
import time


MARKERS = (
    "[initrd] archive validated",
    "[vfs] boot volume ready",
    "[process] init created with PID 1",
    "[init] entered CPL3",
    "[syscall] debug byte from CPL3: 0x49",
)
FATAL = (
    "pvKernel PANIC", "[boot] fatal", "[heap] allocation failed",
    "[exception] page fault", "[exception] general protection fault",
    "[exception] invalid opcode", "[exception] double fault",
    "[exception] device not available", "[syscall] BUG:",
    "[initrd-test]",  # A test-enabled image must not pass this production check.
)


def reached_init(log: str) -> bool:
    offset = 0
    for marker in MARKERS:
        at = log.find(marker, offset)
        if at < 0:
            return False
        offset = at + len(marker)
    return True


def observe(command: list[str], serial: Path, *, timeout: float, settle: float) -> None:
    """Observe a subprocess; always stop it and reject early exit or fatal logs."""
    with tempfile.TemporaryFile() as diagnostics:
        process = subprocess.Popen(command, stdout=diagnostics, stderr=diagnostics)
        deadline = time.monotonic() + timeout
        ready_since = None
        try:
            while True:
                log = serial.read_text(errors="replace") if serial.exists() else ""
                if any(marker in log for marker in FATAL):
                    raise RuntimeError("fatal or test-only diagnostic during production boot")
                if process.poll() is not None:
                    raise RuntimeError(f"QEMU exited before observation completed ({process.returncode})")
                now = time.monotonic()
                if now >= deadline:
                    raise RuntimeError("production boot timed out before verification completed")
                if reached_init(log):
                    if ready_since is None:
                        ready_since = now
                    if now - ready_since >= settle:
                        return
                time.sleep(0.05)
        except BaseException:
            if serial.exists():
                print(serial.read_text(errors="replace"), flush=True)
            diagnostics.seek(0)
            print(diagnostics.read().decode(errors="replace"), flush=True)
            raise
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image", type=Path)
    parser.add_argument("--ovmf-code", required=True, type=Path)
    parser.add_argument("--ovmf-vars", required=True, type=Path)
    parser.add_argument("--qemu", default="qemu-system-x86_64")
    parser.add_argument("--timeout", type=float, default=120)
    args = parser.parse_args()
    if args.timeout <= 2:
        parser.error("timeout must exceed the two-second observation window")
    image = args.image.resolve(strict=True)
    code = args.ovmf_code.resolve(strict=True)
    variables = args.ovmf_vars.resolve(strict=True)
    before = hashlib.sha256(image.read_bytes()).digest()
    try:
        with tempfile.TemporaryDirectory(prefix="pvkernel-boot-") as directory:
            serial = Path(directory) / "serial.log"
            command = [
                args.qemu, "-machine", "q35", "-m", "512M", "-cpu", "max",
                "-display", "none", "-monitor", "none", "-nic", "none", "-no-reboot",
                "-serial", f"file:{serial}",
                "-drive", f"format=raw,file={image},snapshot=on",
                "-drive", f"if=pflash,format=raw,unit=0,file={code},readonly=on",
                "-drive", f"if=pflash,format=raw,unit=1,file={variables},snapshot=on",
            ]
            observe(command, serial, timeout=args.timeout, settle=2)
            print(serial.read_text(errors="replace"), flush=True)
    finally:
        if hashlib.sha256(image.read_bytes()).digest() != before:
            raise RuntimeError("production image was modified during boot verification")
    print("Production release image boot: PASS (PID 1 entered CPL3; image unchanged)")


if __name__ == "__main__":
    main()
