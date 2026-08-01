# pvKernel

pvKernel is an experimental open-source x86_64 operating system kernel
written in Rust and developed as the foundation of pvOS.

> [!WARNING]
> pvKernel is in a very early stage of development. It is not currently
> suitable for production use or as a general-purpose operating system kernel.

![pvKernel boot screen](docs/images/pvkernel-0.0.1.png)

## Current milestone

Version: `0.0.1`

Implemented:

- UEFI boot
- x86_64 kernel entry point
- boot memory map access
- linear framebuffer access
- serial port logging
- panic handler
- CPU halt loop

Not implemented:

- kernel-owned GDT and IDT
- exception handling
- physical memory allocator
- virtual memory manager
- heap
- processes
- scheduler
- user mode
- system calls
- filesystems
- device drivers
- networking

## Build

```bash
cargo build
```

## Run

```bash
cargo run
```

## Current boot flow

```
UEFI firmware
    ↓
rust-osdev bootloader
    ↓
pvKernel ELF
    ↓
kernel_main
    ├── boot memory map
    ├── framebuffer
    └── serial logging
```

The bootloader is currently responsible only for loading the kernel and
providing initial boot information. Kernel subsystems and operating-system
services will be implemented by pvKernel itself.

## Roadmap

### 0.1.0 — CPU foundation

* GDT
* TSS
* IDT
* CPU exception handlers
* page fault handler
* double fault handling
* interrupt controller
* timer

### 0.2.0 — Memory

* physical frame allocator
* virtual memory manager
* kernel heap
* Rust allocation support

### 0.3.0 — Multitasking

* kernel tasks
* context switching
* preemptive scheduler
* synchronization primitives

### 0.4.0 — Userspace

* ring 3
* system calls
* ELF userspace loader
* processes
* init

## License

Licensed under either the MIT License or the Apache License 2.0,
at your option.

## Screenshot

![pvKernel boot screen](docs/images/pvkernel-0.0.1.png)