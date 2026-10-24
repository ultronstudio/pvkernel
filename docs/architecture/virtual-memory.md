# Virtual memory foundation

This document describes pvKernel 0.0.5's physical/virtual memory foundation:
how physical frames are tracked, how the active page table is reached and
mutated, the permission model it enforces, and what is deliberately not yet
implemented. It complements [ARCHITECTURE.md](../../ARCHITECTURE.md), which
covers overall kernel structure and boot order.

## Scope

pvKernel 0.0.5 adds:

* a monotonic boot-time physical frame allocator built from the
  bootloader-provided memory map;
* safe access to the single active x86_64 page table, through a narrow
  unsafe boundary;
* 4 KiB map / unmap / permission-change / translate operations;
* a W^X permission model enforced by the mapping API itself;
* a boot-time smoke test and an optional deterministic post-unmap
  page-fault test.

pvKernel 0.0.5 does **not** implement a kernel heap, Rust `alloc` support,
frame deallocation/reuse, userspace execution, system calls, or an ELF
loader. It does not create any user-accessible or executable mapping
itself. These remain future roadmap items; see [ROADMAP.md](../../ROADMAP.md).

> **0.0.6 update:** `arch::x86_64::userspace` (0.0.6) is the first code to
> actually use `USERSPACE_RANGE_BASE` and `Permissions::USER_DATA`/
> `USER_CODE` below, and the first to execute ring-3 code and a minimal
> `int 0x80` syscall ABI - see
> [docs/architecture/userspace-entry.md](userspace-entry.md) and
> [docs/architecture/syscalls.md](syscalls.md). The rest of this document
> otherwise still describes the 0.0.5 memory foundation accurately.

> **0.0.7 update:** the historical sections below describe the memory
> foundation as introduced in 0.0.5. The fixed kernel heap, global
> allocator, and their layout/security invariants are documented in the
> [0.0.7 heap section](#kernel-heap-pvkernel-007) near the end of this file.

## Canonical addresses and paging mode

x86_64 requires virtual addresses to be in canonical form: bits 63:47 must
all be identical (a sign extension of bit 47). pvKernel validates this with
`x86_64::VirtAddr::try_new` (wrapped as
`memory::paging::canonical_virt_addr`) before performing any address-based
operation, rather than relying on `VirtAddr::new`'s panic-on-invalid-input
behavior.

pvKernel does not enable five-level paging (CR4.LA57); the bootloader
configures standard 4-level paging, which pvKernel verifies at boot
(`memory::validate_paging_prerequisites`) rather than assuming. This check
runs at the very start of `memory::init`, strictly before
`address_space::init` ever interprets the CR3 frame as a four-level table
root (`active_level_4_table` / `OffsetPageTable::new`): if CR4.LA57 is set,
`memory::init` panics immediately instead of reconstructing a page table
under a four-level assumption that the hardware does not actually satisfy.
There is no code path in pvKernel that reaches page-table reconstruction
before this check.

## `BootInfo` and memory-map ownership

`kernel_main` receives `boot_info: &'static mut BootInfo` from the
bootloader. Everything else `kernel_main` still needs from it (currently
only the framebuffer, via `boot_info.framebuffer.take()`) is extracted
*before* `boot_info` is passed into `arch::init`, which takes it **by
value** and forwards it, unchanged, into `memory::init` — which also takes
it by value and does not return it. After that call, there is no live
`&mut BootInfo` anywhere in the kernel: the compiler enforces this, since
`kernel_main`'s own `boot_info` binding was moved away.

Within `memory::init`, the real `MemoryRegions` value is moved out of
`boot_info.memory_regions` with `core::mem::replace`, leaving an
always-empty placeholder (`static mut EMPTY_MEMORY_REGIONS: [MemoryRegion;
0]`) in its place so the field stays valid. The moved-out `MemoryRegions` is
then converted to a `&'static mut [MemoryRegion]` via its safe,
upstream-provided `From` conversion and immediately downgraded to a shared
`&'static [MemoryRegion]` before being handed to `BootFrameAllocator::new`.

This is a deliberate change from pvKernel 0.0.5/0.0.6's original design,
which reconstructed a `'static` slice from `boot_info.memory_regions`'s raw
pointer and length (`core::slice::from_raw_parts`) while `kernel_main`
*also* kept using the same `&'static mut BootInfo` afterward (for the
framebuffer). That was sound in practice (nothing ever mutated
`memory_regions` after the fact), but the exclusivity invariant was
documented, not enforced: nothing in the type system prevented a future
change from reading or writing `boot_info.memory_regions` while the frame
allocator's cached slice was still considered authoritative. The current
design removes that possibility structurally: the frame allocator's memory
map is *moved*, not aliased, and the only other owner (`boot_info` itself)
is unreachable by the time the move happens.

## Bootloader-established mappings

The `rust-osdev` bootloader builds the initial page table before jumping to
`kernel_main`, and pvKernel does not replace it — only adds to it. As of
0.0.5, `kernel/src/main.rs` requests one additional mapping through a
custom `BootloaderConfig`:

```rust
let mut config = BootloaderConfig::new_default();
config.mappings.physical_memory = Some(Mapping::Dynamic);
```

This maps all usable physical memory at a bootloader-chosen virtual offset
(`BootInfo::physical_memory_offset`), which is required for
`OffsetPageTable` to translate page-table frame addresses into accessible
virtual addresses. Without it, `arch::x86_64::memory` has no way to reach
the active page table at all. All other default mappings (kernel image,
kernel stack, boot info, framebuffer) keep the bootloader's default dynamic
placement; pvKernel does not currently change or rely on their specific
addresses.

## Physical frame allocator (`frame_allocator::BootFrameAllocator`)

`BootFrameAllocator` is a monotonic allocator built once from the
bootloader's `MemoryRegions`: it skips non-usable regions, aligns starts up
and ends down to 4 KiB boundaries, and keeps a region index plus next-frame
address. Allocation advances this cursor without rebuilding or rescanning a
frame iterator. The total frame count remains an occasional O(number of
regions) diagnostic scan.

**Limitations, by design in this milestone:**

* frames are never returned or reused — `unmap_page` returns the physical
  frame to the caller, but nothing hands it back to the allocator;
* only 4 KiB frames are allocated; pvKernel never creates 2 MiB/1 GiB huge
  pages itself;
* allocation is monotonic and does not reclaim frames; its total traversal
  cost is O(number of memory regions + number of allocated frames), rather
  than repeatedly rescanning all prior frames.

Heap object memory reuse is provided by the separate kernel heap allocator;
that does not return its backing frames to this allocator.

## Active page-table access (`address_space::KernelAddressSpace`)

pvKernel owns exactly one `x86_64::structures::paging::OffsetPageTable`,
constructed from the CR3-resident level-4 table plus the physical-memory
offset mapping described above. It is stored, together with the
`BootFrameAllocator`, inside a single `KernelAddressSpace` behind a
`spin::Mutex`, itself behind a `spin::Once`:

```rust
static KERNEL_ADDRESS_SPACE: Once<Mutex<KernelAddressSpace>> = Once::new();
```

The only way to reach it is `memory::with_kernel_address_space(f)`, which
runs `f` with the lock held and hardware interrupts disabled
(`x86_64::instructions::interrupts::without_interrupts`), so an interrupt
handler can never observe or contend for a half-updated page table on the
same CPU. pvKernel is currently single-CPU (no SMP/AP bring-up), so this is
sufficient; a future SMP milestone would need to revisit this (see
[Future work](#future-work-and-known-limitations)).

### `active_level_4_table` safety invariants

Reconstructing `&'static mut PageTable` from the physical address in CR3
requires five invariants, documented at its definition
(`address_space.rs`):

1. **Four-level paging is active (CR4.LA57 clear).** Verified by
   `memory::validate_paging_prerequisites`, which runs before this function
   is ever reachable (see [Canonical addresses and paging
   mode](#canonical-addresses-and-paging-mode) above) and panics instead of
   letting a five-level CR3 root be misinterpreted as a four-level one.
2. **A valid physical-memory mapping exists.** Guaranteed by requesting
   `Mapping::Dynamic` for `physical_memory` in the bootloader config (see
   above); `memory::init` panics loudly if the mapping is absent instead of
   dereferencing a bogus offset.
2. **The CR3 frame is reachable through that mapping.** True by
   construction: the physical-memory mapping covers all usable physical
   memory, and CR3 always points at a real, currently-active page-table
   frame.
3. **No other `&mut` reference to the same table exists concurrently.**
   Enforced by only ever constructing this reference once, at
   `memory::init` time, and storing the resulting `OffsetPageTable` (which
   owns that reference) inside the single, lock-protected
   `KERNEL_ADDRESS_SPACE`. No other code path re-derives a fresh reference
   from CR3.
4. **The reference does not outlive the mapping.** The physical-memory
   mapping is permanent for the kernel's lifetime (the bootloader never
   unmaps it), so a `'static` reference is sound.

## Permission model (`paging::Permissions`) and W^X

Every mapping created through this module specifies:

```rust
pub struct Permissions {
    pub writable: bool,
    pub executable: bool,
    pub user_accessible: bool,
}
```

with `KERNEL_DATA` (writable, non-executable), `KERNEL_READ_ONLY`
(read-only, non-executable), and `KERNEL_EXECUTABLE` (read-only,
executable) constants. `Permissions::to_flags` **refuses** to produce a
mapping that is both `writable` and `executable`: this is pvKernel's W^X
policy, enforced at the point flags are computed, not left to caller
discipline. Attempting it returns `MemoryError::InvalidPermissions` instead
of silently producing an insecure mapping.

Mappings are supervisor-only (`USER_ACCESSIBLE` unset) unless
`user_accessible` is explicitly requested, and non-executable
(`NO_EXECUTE` set) unless `executable` is explicitly requested — the
default shape of any mapping is the most restrictive one.

`KERNEL_EXECUTABLE` exists for a future milestone (there is no ELF loader
yet, so pvKernel 0.0.5 never actually creates an executable mapping
itself); it is already part of the model so a later milestone does not need
to redesign the permission type.

## NX (`EFER.NXE`)

pvKernel's bootloader (`bootloader-x86_64-common`) sets `EFER.NXE` before
jumping to `kernel_main`, which makes the page-table `NO_EXECUTE` bit
meaningful. `memory::validate_paging_prerequisites`, at the start of
`memory::init`, requires `IA32_EFER.NXE` to be active and panics with an
explicit prerequisite failure before page-table initialization if it is
not. The existing W^X mapping policy therefore cannot silently operate
without execute-disable support.

## Supervisor write protection (`CR0.WP`)

`memory::validate_paging_prerequisites` also ensures `CR0.WP` is set before
address-space initialization proceeds. If the bootloader did not set it,
pvKernel enables it and verifies the resulting state. This is required
because without `CR0.WP`, supervisor code can write through mappings marked
read-only, bypassing the page protections the kernel relies on. The
deterministic `supervisor-write-protection-test` QEMU scenario verifies the
invariant by attempting a CPL0 write to a present, supervisor-only,
read-only page and accepting only the matching protection-violation fault.

## Address translation (`paging::translate`)

`translate(mapper, addr)` returns a `Translation`:

* `Mapped { phys_addr, flags }` — an ordinary 4 KiB mapping;
* `HugePage { phys_addr }` — mapped through a 2 MiB/1 GiB huge page (never
  created by pvKernel itself, but distinguished in case the bootloader's
  own mappings use one);
* `NotMapped` — no translation exists;
* `Unsupported` — the page-table entry pointed at an invalid physical
  address.

Like `canonical_virt_addr`, this never panics on an ordinary unmapped
address — callers get a typed result instead.

## Map / unmap operations (`paging::map_page` / `paging::unmap_page` / `paging::update_permissions`)

* `map_page` allocates any missing intermediate page-table levels from the
  supplied frame allocator, maps a single 4 KiB page to a single 4 KiB
  frame with the requested `Permissions`, and flushes the mapped page's TLB
  entry.
* `unmap_page` unmaps a page, flushes its TLB entry, and returns the
  physical frame it was mapped to. As noted above, the frame is **not**
  returned to any allocator — callers must not treat it as reusable in this
  milestone.
* `update_permissions` changes the flags of an existing mapping and
  flushes its TLB entry. It never sets the `GLOBAL` flag, so there is no
  cross-CPU TLB-shootdown requirement to uphold (relevant only once SMP
  exists).

All three return a project-specific `MemoryError` (not the raw `x86_64`
crate error types) so callers do not need to match on multiple upstream
enums: `NonCanonicalAddress`, `Unaligned`, `AlreadyMapped`, `NotMapped`,
`FrameAllocationFailed`, `ParentEntryHugePage`, `InvalidPermissions`, and
`InvalidFrameAddress`.

## Temporary mapping range and userspace reservation

* `memory::TEMP_MAPPING_BASE` (`0xFFFF_9000_0000_0000`) is a canonical,
  page-aligned virtual address pvKernel's own boot-time tests use for
  scratch mappings. It is **internal and unstable**: nothing outside this
  module's tests may assume it stays at this address across releases. Every
  test using it first confirms the address is unmapped
  (`Translation::NotMapped`) before mapping it, so an unexpected collision
  with a bootloader-chosen dynamic mapping fails loudly (a panic) instead of
  silently corrupting memory.
* `memory::USERSPACE_RANGE_BASE` (`0x0000_6000_0000_0000`) was a **policy
  reservation only** as of 0.0.5: that release did not populate, map, or
  otherwise use this range. Starting in 0.0.6,
  `arch::x86_64::userspace` maps its (test-only) code, guard, and stack
  pages within it; see
  [docs/architecture/userspace-entry.md](userspace-entry.md).

## Guard pages and huge-page policy

pvKernel 0.0.5 does not create guard pages (there is no growable kernel
stack or heap yet to guard) and never creates a huge-page (2 MiB/1 GiB)
mapping itself. If a caller's page happens to fall under a parent
page-table entry that is already part of a huge page (for example, one of
the bootloader's own mappings), `map_page`/`unmap_page`/`update_permissions`
return `MemoryError::ParentEntryHugePage` rather than corrupting or
silently reinterpreting that mapping.

## Concurrency and interrupt assumptions

`with_kernel_address_space` disables maskable hardware interrupts for the
duration of the closure and holds a `spin::Mutex`. This is sufficient
because pvKernel 0.0.5 is single-CPU: there is no AP (application
processor) bring-up, so there is no possibility of true concurrent access
from another core. The `spin::Mutex` primarily guards against reentrancy
from an interrupt handler on the *same* CPU (which `without_interrupts`
already prevents) and keeps the API honest about being a shared resource,
in preparation for eventual SMP support.

## What pvKernel 0.0.5 does not execute

pvKernel 0.0.5 does not execute ring-3 code, does not load pvOS executables
or any ELF binary, and does not define a stable syscall ABI. Every mapping
created in this milestone is supervisor-only and is created either by the
boot-time smoke test or the optional post-unmap fault test - there is no
general-purpose "map memory for a subsystem" caller yet.

> Starting in 0.0.6, `arch::x86_64::userspace` (test-only, behind Cargo
> features) does execute ring-3 code and a minimal, provisional `int 0x80`
> syscall ABI - see [docs/architecture/userspace-entry.md](userspace-entry.md)
> and [docs/architecture/syscalls.md](syscalls.md). There is still no ELF
> loader, no process model, and no general-purpose "map memory for a
> subsystem" caller.

## Boot-time tests

See [docs/development/memory-testing.md](../development/memory-testing.md)
for what the boot-time smoke test and the optional post-unmap page-fault
test verify, and how to run them.

## Kernel heap (pvKernel 0.0.7)

The kernel reserves this fixed canonical virtual range:

```text
0xffff90000ffff000  lower unmapped guard page
0xffff900010000000  HEAP_START
                    1 MiB heap (256 x 4 KiB pages)
0xffff900010100000  HEAP_END / upper unmapped guard page
```

The range is separate from the temporary mapping probe at
`0xffff900000000000` and the low userspace test range. At boot, every heap
and guard page is checked as unmapped before mapping. A collision or mapping
failure is fatal; allocator initialization does not proceed with a partial
heap. The virtual range is checked for canonicality, page alignment, and
arithmetic overflow.

Heap pages use `PRESENT | WRITABLE | NO_EXECUTE`, with
`USER_ACCESSIBLE` clear. This depends on the existing validated 4-level
paging mode, `EFER.NXE`, `CR0.WP`, and W^X mapping policy. Both adjacent guard
pages remain unmapped. Representative first, middle, and last pages and both
guards are checked before the global allocator becomes available.

Initialization order is: disable interrupts; initialize GDT/TSS and IDT;
validate paging prerequisites; initialize the stateful frame allocator and
active page table; map and verify the heap; initialize its global allocator;
then proceed with PIC/PIT and later boot tests. Allocations before allocator
initialization return null and flow to the deterministic OOM handler.

The allocator is a repository-owned first-fit bitmap. Its static occupancy
bitmap tracks 16-byte heap units, and a second bitmap records allocation
starts. It aligns requested addresses and marks the required units; freeing
clears those bits, so adjacent free units are immediately reusable as a larger
run. The static metadata avoids raw pointer dereferences and does not
recursively allocate from the heap. A 1 MiB heap uses 16 KiB of bitmap state.
A spin mutex protects the bitmaps; each allocator critical section disables
maskable interrupts to avoid same-CPU IRQ reentrancy and
deadlock. The kernel is still single-CPU, so SMP synchronization is not
claimed. OOM reports size and alignment using allocation-free serial
formatting and halts (or exits as failure in QEMU test mode).

Physical backing frames allocated for heap pages transfer to the heap and
remain owned by it for the kernel lifetime. `Box`, `Vec`, and `String`
deallocation returns space to the heap allocator; it does not return frames
to the monotonic physical frame allocator. Heap mapping failures after frame
allocation are fatal and do not attempt unsafe rollback.

## Future work and known limitations

* No general physical-frame deallocation/reuse — heap backing frames remain
  owned for kernel lifetime.
* The heap is fixed-size (1 MiB); there is no dynamic heap growth, per-process
  heap, or slab allocator.
* No SMP-aware locking beyond `without_interrupts` + `spin::Mutex` (correct
  for a single CPU; would need revisiting for multi-core support).
* No huge-page creation, and no support for unmapping/remapping a range
  that spans multiple page sizes.
