# Debugging

## Serial logging

pvKernel writes structured diagnostics to COM1 through the `kprint!`/
`kprintln!` macros (`kernel/src/drivers/serial.rs`). The root runner attaches
COM1 to the QEMU process's standard output via `-serial stdio`, so
`cargo run` prints the boot log directly to your terminal alongside QEMU's
own firmware messages.

## QEMU output and `-no-reboot`

The runner passes `-no-reboot` to QEMU. Without it, a triple fault (an
unrecoverable CPU fault, for example a double fault that itself faults)
would cause QEMU to silently reset and reboot the guest, which looks like the
kernel simply restarting from the UEFI firmware messages with no panic or
exception output. With `-no-reboot`, QEMU instead exits (and, for `-serial
stdio`, closes the terminal's serial output) when such a fault occurs. If you
run `cargo run` and observe the boot messages appearing more than once, treat
that as a triple fault, not a design goal.

## Interpreting a silent reset

If QEMU's window closes or the process exits shortly after boot with no
fatal-exception message printed, check for:

- a fault occurring before serial output is initialized (unlikely in current
  code, since serial is initialized first in `kernel_main`);
- a fault inside a fault handler itself, which can escalate to a double or
  triple fault instead of printing the expected single diagnostic block.

## Breakpoint test

Every normal boot (`cargo run`) triggers one controlled breakpoint:

```
[test] triggering breakpoint
[exception] breakpoint
... (interrupt stack frame) ...
[test] breakpoint returned successfully
```

If `[test] breakpoint returned successfully` is missing, the breakpoint
handler did not return correctly; this indicates a GDT/IDT configuration
problem, not a normal breakpoint.

## Page-fault test

Run:

```bash
cargo run --features fault-tests
```

Expected output, after the breakpoint test:

```
[test] WARNING: triggering an intentional page fault
[exception] page fault
faulting address (CR2): 0xdeadbeef
error code: PageFaultErrorCode(...)
... (interrupt stack frame) ...
```

The kernel halts in the page-fault handler; QEMU does not restart (this was
verified against `-no-reboot`). The faulting address `0xdead_beef` is chosen
because it is not mapped in pvKernel's current address space, so
dereferencing it reliably produces a page fault rather than reading
unrelated memory.

## Debugger integration

Direct GDB/LLDB integration (for example, via QEMU's `-s -S` flags) has not
been verified against the current runner and is not documented here.
Debugger integration is not yet part of the root runner and is considered
incomplete for this release.

## WSLg display behavior

On WSL2 with WSLg, GTK-based display windows (including QEMU's `gtk`
display) can render incorrectly or fail to appear under Wayland. The runner
sets `GDK_BACKEND=x11` when launching QEMU to force GTK to use WSLg's X11
compatibility layer. This is applied unconditionally by the runner; it is a
no-op on systems where GTK does not use the Wayland backend, so it does not
need to be removed on native Linux.
