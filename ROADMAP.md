# Roadmap

This roadmap describes the intended direction for pvKernel. It is a planning
document, not a commitment. Milestones, scope, and ordering may change as the
project learns more about what each step actually requires.

## 0.0.2 — CPU Exception Foundation

* kernel-owned GDT and TSS
* dedicated double-fault IST stack
* kernel-owned IDT
* breakpoint, invalid opcode, general protection fault, page fault, and
  double fault handlers
* controlled boot-time breakpoint test
* optional page-fault test behind a Cargo feature

## 0.0.3 — CPU Exception Correctness

Corrections found during review of the 0.0.2 release. No new subsystems are
added in this milestone.

* writable (`UnsafeCell`-backed) double-fault IST stack storage
* clarified documentation of the IST index (Rust array index vs. hardware
  IST slot encoding)
* confirmed CI toolchain declaration installs `rust-src`
* translated remaining non-English source comments

## 0.0.4 — Hardware Interrupts and Timer

* a legacy 8259 PIC driver: IRQ vector remapping, masking/unmasking, EOI,
  and spurious IRQ7/IRQ15 detection
* IDT handlers for all 16 remapped IRQ vectors (32-47)
* a PIT channel 0 periodic timer with an atomic tick counter
* a boot-time verification that hardware interrupts are safely enabled and
  repeated timer ticks are delivered
* a fatal-halt vs. interrupt-enabled idle distinction
* deterministic, headless QEMU tests (timer delivery and an expected page
  fault) with a pass/fail exit status, and CI coverage for them

APIC support, keyboard input, and any other IRQ line beyond the timer are
out of scope for this milestone and remain future roadmap items.

## 0.0.5 — Physical Memory Allocator

* parsing the bootloader-provided memory map into a frame allocator
* frame allocation and deallocation primitives

## 0.0.6 — Virtual Memory and Kernel Heap

* kernel-owned page tables
* a kernel heap
* Rust `alloc` support

## 0.0.7 — Kernel Tasks and Scheduler

* kernel-level tasks
* context switching
* a preemptive scheduler

## 0.0.8 — Ring 3 and System Calls

* user mode execution
* a system call interface

## 0.0.9 — ELF Userspace Loader and Init

* loading ELF userspace binaries
* a minimal init process

## 0.0.10 — VFS and Initial Ramdisk

* a virtual filesystem layer
* initial ramdisk support

## 0.1.0 — First pvOS-Ready Kernel Foundation

A kernel foundation capable of supporting early pvOS userspace development.
This milestone is not a production-ready or general-purpose release.

---

Roadmap items may be reordered, split, or removed as implementation
experience accumulates. No dates are attached to these milestones.
