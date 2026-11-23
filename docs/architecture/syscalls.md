# pvOS userspace syscall ABI v1

This document specifies the initial Rust userspace ABI implemented by
pvKernel's `int 0x80` gate. The supported Rust wrappers are in
[`userspace/userlib`](../../userspace/userlib/). The first compatibility
checkpoint is pvKernel 0.1.0; ABI v1 will keep existing syscall numbers,
argument meanings, return encoding, and process behavior compatible from that
release onward. Additive syscall numbers do not change the ABI version.
Incompatible changes require a new ABI version.

This is a pvOS ABI. It does not claim POSIX or Linux compatibility.

## Entry and registers

The only entry mechanism is software interrupt `int 0x80`; `SYSCALL`/`SYSRET`
are not implemented.

* `RAX`: syscall number on entry; result on return.
* `RDI`, `RSI`, `RDX`, `R10`, `R8`, `R9`: up to six arguments in that order.
* Other general-purpose registers are preserved by the entry stub.

The IDT gate is callable from ring 3. The processor switches to the current
task's supervisor-only kernel-entry stack before the stub calls Rust. Kernel
entry stacks are 64 KiB with unmapped guards; this accommodates synchronous
process creation and ELF validation on the syscall path.

## Return and errors

Results use `RAX`:

* bit 63 clear: success; the lower bits contain the result.
* bit 63 set: error; the remaining bits are an `Error` number.

| Number | Error | Meaning |
| ---: | --- | --- |
| 1 | `Unsupported` | Unknown syscall number. |
| 2 | `InvalidArgument` | Invalid length, path encoding, or argument. |
| 3 | `PermissionDenied` | Reserved for a denied operation. |
| 4 | `InvalidContext` | No process-owned userspace task is current. |
| 5 | `BadDescriptor` | Descriptor is closed or belongs to another process. |
| 6 | `NotFound` | Path or volume does not exist. |
| 7 | `InvalidOperation` | Directory used as a file or invalid process state. |
| 8 | `NoResources` | Bounded kernel table or process capacity is full. |
| 9 | `UserMemory` | User range is unmapped, inaccessible, or overflows. |
| 10 | `NotChild` | PID is not a direct child of the caller. |
| 11 | `IoFailure` | Filesystem provider failed. |

## Syscalls

All numbers below are compiled into the kernel. Pointers refer to the current
process and are copied only through the checked `copy_from_user` or
`copy_to_user` path. The maximum path length is 255 bytes; paths are
volume-qualified UTF-8 strings with an explicit byte length. Read and console
write calls are capped at 512 bytes per call.

| Number | Name | Arguments | Success result |
| ---: | --- | --- | --- |
| 0 | `ABI_VERSION_QUERY` | — | ABI version (`1`). |
| 1 | `DEBUG_WRITE_BYTE` | `RDI`: byte value | `0`; diagnostic output, retained for tests and boot diagnostics. |
| 2 | `TIMER_TICKS` | — | PIT tick counter. |
| 3 | `GETPID` | — | Current process PID. |
| 4 | `EXIT` | `RDI`: signed 32-bit status | Never returns; ends the current process. |
| 5 | `OPEN` | `RDI`: path pointer, `RSI`: path byte length | Read-only descriptor, numbered from `3`. |
| 6 | `READ` | `RDI`: descriptor, `RSI`: output pointer, `RDX`: maximum bytes | Bytes copied; `0` at EOF. |
| 7 | `CLOSE` | `RDI`: descriptor | `0`. |
| 8 | `CONSOLE_WRITE` | `RDI`: byte pointer, `RSI`: byte length | Bytes written to the serial console. |
| 9 | `SPAWN` | `RDI`: path pointer, `RSI`: path byte length | PID of the new direct child. |
| 10 | `WAIT_CHILD` | `RDI`: child PID, `RSI`: writable `i32` status pointer | `0` while running; `1` after writing status and reaping. |

`OPEN` accepts regular files only. Directories return `InvalidOperation`.
`Boot:` currently provides immutable initrd files, so descriptors are
read-only and are not inherited by children. Each process can hold up to eight
descriptors. Process exit closes all remaining descriptors.

`SPAWN` loads a static x86_64 ELF64 `ET_EXEC` from the VFS using the kernel's
checked loader, creates a child process, and returns its PID. It does not
replace the caller's image. `WAIT_CHILD` is non-blocking: callers retry while
it returns `0`. A completed child is reaped only after its status has been
copied successfully into the caller's writable memory. Exit statuses are
preserved; a faulted child is reported as `-128 - exception_vector`.

`EXIT` records the status and hands off from the exiting task. A timer switch
publishes the process as a Zombie from another task's stack. The parent can
then observe the status and reap the child. No scheduler or process-table lock
is held while VFS lookup, ELF validation, user copying, or console output runs.

## Rust userspace library

`pvkernel-user` is a `no_std` crate with wrappers for ABI version query, PID,
read-only `File`, console output, child spawn, non-blocking wait, and exit.
Wrappers reject over-limit buffers before issuing the syscall. Raw pointer
construction remains the responsibility of safe Rust references passed to
the library; the kernel independently validates every address.

## Deliberate limits

ABI v1 does not provide `SYSCALL`/`SYSRET`, blocking waits, stdin or keyboard
input, writable files, disk storage, mmap, a user heap, signals, IPC, threads,
or dynamic linking. Static ELF64 is the current bootstrap process image
format, not the final pvOS application format.

The deterministic `userspace-test` syscall namespace (`>= 0x1000`) is
test-only and is not part of ABI v1.
