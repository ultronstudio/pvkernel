# Roadmap

This roadmap describes the intended direction for pvKernel. It is a planning
document, not a commitment. Milestones, scope, and ordering may change as the
project learns more about what each step actually requires.

## 0.0.2 — CPU Exception Foundation

- kernel-owned GDT and TSS
- dedicated double-fault IST stack
- kernel-owned IDT
- breakpoint, invalid opcode, general protection fault, page fault, and
  double fault handlers
- controlled boot-time breakpoint test
- optional page-fault test behind a Cargo feature

## 0.0.3 — Hardware Interrupts and Timer

- PIC or APIC initialization
- enabling maskable hardware interrupts
- a timer interrupt source
- basic IRQ dispatch

## 0.0.4 — Physical Memory Allocator

- parsing the bootloader-provided memory map into a frame allocator
- frame allocation and deallocation primitives

## 0.0.5 — Virtual Memory and Kernel Heap

- kernel-owned page tables
- a kernel heap
- Rust `alloc` support

## 0.0.6 — Kernel Tasks and Scheduler

- kernel-level tasks
- context switching
- a preemptive scheduler

## 0.0.7 — Ring 3 and System Calls

- user mode execution
- a system call interface

## 0.0.8 — ELF Userspace Loader and Init

- loading ELF userspace binaries
- a minimal init process

## 0.0.9 — VFS and Initial Ramdisk

- a virtual filesystem layer
- initial ramdisk support

## 0.1.0 — First pvOS-Ready Kernel Foundation

A kernel foundation capable of supporting early pvOS userspace development.
This milestone is not a production-ready or general-purpose release.

---

Roadmap items may be reordered, split, or removed as implementation
experience accumulates. No dates are attached to these milestones.
