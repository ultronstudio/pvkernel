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

## 0.0.5 — Virtual Memory Foundation

* parsing the bootloader-provided memory map into a monotonic physical
  frame allocator
* safe, narrowly-scoped access to the active x86_64 page table
* 4 KiB map / unmap / permission-change / translate operations
* a W^X permission model enforced by the mapping API itself
* verification of the NX bit and current paging mode
* a boot-time mapping smoke test and an optional deterministic post-unmap
  page-fault test

> This milestone was expanded from an earlier plan that split "physical
> memory allocator" (0.0.5) and "virtual memory and kernel heap" (0.0.6)
> into two releases. Implementation experience showed frame allocation and
> page-table access are not independently useful, so they are combined
> here; the kernel heap and `alloc` support move to 0.0.7 below (further
> renumbered from 0.0.6; see the 0.0.6 note below). Frame deallocation/
> reuse, guard pages, and userspace/user-accessible mappings remain future
> work (userspace/user-accessible mappings arrive in 0.0.6 immediately
> below).

## 0.0.6 — Userspace Entry and Syscall Foundation

* dedicated ring-3 (CPL3) code/data GDT descriptors and a dedicated
  TSS.RSP0 ring-transition kernel stack
* the first CPL0 → CPL3 transition, via `iretq`
* a minimal, provisional, scalar-only `int 0x80` syscall ABI (no
  `SYSCALL`/`SYSRET`, no user-pointer arguments)
* `USER_DATA`/`USER_CODE` page permissions, reusing the existing W^X
  mapping API
* three deterministic, headless QEMU tests exercising the syscall
  boundary, an expected CPL3 privilege-instruction fault, and an expected
  CPL3 supervisor-page fault

> **Reordering note:** this milestone was originally planned as "Kernel
> Heap and Rust `alloc` Support" (see below), with ring 3 and system calls
> planned for 0.0.8. Implementation experience with the GDT/TSS and IDT
> work from 0.0.2-0.0.5 made a first, deliberately minimal CPL0 → CPL3
> transition and bootstrap syscall ABI a natural next step ahead of the
> kernel heap, so it was pulled forward and the following milestones were
> renumbered accordingly. This milestone explicitly does **not** add
> `SYSCALL`/`SYSRET`, processes, threads, a scheduler, an ELF loader, or
> user-pointer dereferencing (`copy_from_user`/`copy_to_user`) - those
> remain future work, some of it now folded into 0.0.9 below.

## 0.0.7 — Kernel Heap and Rust `alloc` Support

* stateful monotonic physical frame allocation
* a fixed 1 MiB supervisor-only, writable, NX heap with unmapped guards
* a first-fit bitmap allocator using 16-byte units, allocation-start metadata,
  and reusable/coalescing adjacent free runs
* Rust `alloc` support for `Box`, `Vec`, and `String`
* deterministic heap, OOM, and guard-page QEMU tests

Physical backing frames remain owned by the heap for the kernel lifetime;
freeing heap objects returns memory to the heap allocator, not the physical
frame allocator. Dynamic heap growth and general physical-frame reclamation
remain future work.

## 0.0.8 — Scheduler + Kernel Tasks

* bounded, single-core, ring-0 kernel tasks with dedicated guarded stacks
* generation-aware task IDs and safe task lifecycle/slot reuse
* complete x86_64 timer-interrupt context switching and deterministic
  round-robin preemption
* QEMU coverage for scheduler operation, true timer preemption, stack guards,
  and capacity/reuse

## 0.0.9 — Processes and Userspace Address Spaces (implemented; unreleased)

* generation-safe ProcessId handles, monotonic PIDs, bounded process table,
  parent identity, Zombie/reap lifecycle, and process snapshots
* audited per-process address spaces and CR3 switching while preserving
  supervisor-only kernel-global mappings
* dedicated user and kernel-entry stacks, TSS.RSP0 switching, and true PIT
  preemption from CPL3
* checked static ELF64 `ET_EXEC` loading with W^X, BSS/page zeroing, and an
  embedded smoke ELF before VFS/initrd exists
* experimental `getpid`/`exit`, central checked user-copy, and CPL3
  #PF/#GP/#UD containment
* deterministic QEMU tests for process preemption, address-space isolation,
  capacity/reuse, ELF validation, user-copy, guard faults, and containment
* no stable userspace ABI is implied; first meaningful compatibility
  checkpoint remains 0.1.0

## 0.0.10 — VFS and Initial Ramdisk

* VFS foundation
* initrd and root filesystem foundation

## 0.1.0 — First pvOS-Ready Kernel Foundation

A kernel foundation capable of supporting early pvOS userspace development.
This milestone is not a production-ready or general-purpose release.

---

Roadmap items may be reordered, split, or removed as implementation
experience accumulates. No dates are attached to these milestones.
