# pvKernel 0.0.10 pre-release audit

**Audit date:** 2026-10-25
**Subject:** current uncommitted pvKernel 0.0.10 working tree
**Verdict: READY FOR RC1 REVIEW**

This is a source audit of the current 0.0.10 implementation, including the
volume-aware namespace correction. The tree was not committed or tagged during
this review. The conclusion is based on manual invariant tracing, the local
validation listed below, and the recorded QEMU runs. Tests corroborate the
review; they do not replace it.

## Scope and method

The review covered the complete 0.0.9-to-0.0.10 change surface: the pinned
bootloader image API and `BootInfo` transport, boot mappings, CPIO `newc`, the
path parser and VFS/provider boundary, process construction and task
publication, the existing ELF loader, the userspace init artifact, QEMU
scenario selection, local/release image production, CI and documentation.
Unsafe additions, panic/expect sites, allocation bounds and lock interactions
were inspected separately. The bootloader implementation inspected was
`bootloader` / `bootloader_api` 0.11.17 from the locked Cargo registry source.

The review is scoped to the current x86_64 four-level, single-CPU,
interrupt-preemptive kernel. It does not certify SMP, persistent storage,
additional filesystem providers, executable formats, or a stable userspace
ABI.

## Release scope

0.0.10 provides a fixed-capacity, read-only VFS volume registry with one
temporary `Boot:` volume backed by bootloader-delivered CPIO `newc`. The
canonical boot executable is `Boot:\\pvOS\\System\\Init`; it is a separately
built static ELF64 `ET_EXEC` image, loaded by the existing checked loader and
published as parentless numeric PID 1.

The implementation does not add a persistent `C:`, a block layer or disk
filesystem, NTFS, exFAT, FAT32, pvFS, general filesystem syscalls or file
descriptors, `fork`, general `exec`, dynamic linking, PVX, application bundles,
packages, a shell, or a stable userspace ABI. The initrd is temporary boot
storage, not a permanent pvOS root filesystem. No stable release or RC tag was
created by this audit.

## Findings and disposition

| Severity | Finding | Evidence and impact | Recommendation / disposition |
| --- | --- | --- | --- |
| P0 | None | No catastrophic boot, isolation, or release-image defect found. | None. |
| P1 | None | No exploitable parser, mapping-lifetime, ELF-policy, or process-publication defect found in the reviewed paths. | None. |
| P2 | None | No significant unresolved correctness or security issue blocks RC review. | None. |
| P3 | Parent-component normalization retained a trailing separator in canonical serialization. | In `kernel/src/fs/path.rs`, popping a non-final component shortened the byte length to the next component start, which includes the separator before that component. Lookup components were unaffected, but equivalent paths had different canonical strings. | **Fixed during audit.** The pop also removes that separator when a component remains; host tests cover terminal `..` to a non-root directory and to the volume root. |
| P3 | Security-policy test used a filename that Windows cannot create. | `scripts/tests/test_check_security_policy.py` created `.github/workflows/::error::x.yml`; `:` is not a valid Windows filename character, so one host test errored without testing the product policy. | **Fixed during audit.** The test now uses `%` and `]`, valid on Windows, and verifies sanitized output. All 66 Python tests pass on Windows. |
| P3 | Roadmap/process documentation still described 0.0.10 as in progress and VFS/initrd as out of scope. | `ROADMAP.md` and the process architecture non-goals had not been refreshed after the runtime-complete milestone. | **Fixed during audit.** 0.0.10 is marked implemented but unreleased, and the process document now lists only the remaining filesystem-related exclusions. |
| P3 | Initrd mapping is writable in the boot page tables. | Bootloader 0.11.17 maps ramdisk pages supervisor read/write and NX. The kernel exposes only a shared immutable slice and current parser/provider code only reads it, but the page permission does not enforce read-only access. This is a privileged-kernel software invariant, not a user-writable mapping. | Accepted for 0.0.10. Keep all consumers read-only; consider page-table write protection as defense in depth when it can be added without changing boot mapping ownership. |

No P0, P1, or P2 findings remain. The resolved P3 changes are present in the
audited tree. The remaining P3 item is a documented hardening opportunity and
does not expose a user-controlled write path in this single-core kernel.

## BootInfo ramdisk provenance, mapping and lifetime

The local and release image path is the root `build.rs`: Cargo builds the
kernel and `userspace/init` for `x86_64-unknown-none`; the script constructs a
deterministic archive; `bootloader::UefiBoot::new(kernel)` plus
`UefiBoot::set_ramdisk(path)` passes the archive as a separate image input;
`create_disk_image` creates the UEFI image. Inspection of the pinned 0.11.17
source confirms `set_ramdisk` stores a separate `ramdisk` file in the generated
disk image. The UEFI loader loads that file into RAM and passes its physical
start and length to the common bootloader layer. The common layer maps those
physical frames into the kernel's dynamic mapping arena and writes the
**mapped virtual slice start**, not the original physical address, to
`BootInfo.ramdisk_addr`, with its byte length in `ramdisk_len`.

`kernel_main` copies the two scalar metadata fields before moving the unique
`BootInfo` reference into `arch::init`. Memory initialization records the
ramdisk range in `BootMappingInventory`, constructs/audits the active kernel
address space and preserves the upper-half boot mappings. The audit requires
the range to be nonempty, within the configured dynamic arena, canonical,
mapped page by page, and supervisor-only. Only after `arch::init` returns does
`BootRamdisk::from_mapping` validate presence, nonzero length, `usize` and
`isize` representability, checked `start + len`, canonical first/last byte,
and arena containment before `slice::from_raw_parts` is called. The code uses
the BootInfo virtual address directly; it does not reinterpret it as physical.

The lifetime is boot lifetime: bootloader memory-map accounting reserves the
backing ramdisk pages, the kernel does not reclaim them, and the audited kernel
root retains the dynamic mapping. `BootInfo` is consumed before the borrowed
archive is constructed, but the mapping is independent of the metadata object.
`BootRamdisk` owns only a `&'static [u8]`; `Archive`, `InitrdFs`, the static
provider and VFS all borrow that slice. There is no mutable Rust reference and
no current write path. The bootloader's page permission is nevertheless
supervisor RW/NX, including the physical-map alias; see the accepted P3
hardening note above. The unsafe slice conversion's preconditions are explicit
and tied to the pinned bootloader API plus the completed mapping audit.

Absent address, zero length, endpoint overflow, noncanonical address, and
out-of-arena metadata return typed errors before slice construction. Normal
boot treats absence/corruption as fatal with serial diagnostics and halt; the
QEMU test configuration exits with its expected result only for its selected
failure. There is no embedded-ELF fallback.

## CPIO `newc` parser

`kernel/src/fs/initrd/cpio.rs` validates the entire archive before it creates
an `Archive` view. It requires the six-byte `070701` magic and a bounded
110-byte header; decodes each of the 13 eight-byte hexadecimal fields
individually; rejects invalid digits and nonzero checksum fields; and uses
checked addition, conversion and four-byte alignment. Names must fit the
archive, be UTF-8, terminate with exactly one final NUL, and have zero alignment
padding. File data and trailing alignment padding are independently bounded
and checked. No unaligned struct casts or pointer arithmetic are used.

The supported entry subset is regular files with link count 1 and empty
directories. Other node types, including symlinks and devices, are rejected.
Archive names must be relative, canonical slash-separated paths with no empty,
`.` or `..` component; components are at most 63 UTF-8 bytes, total names at
most 255 bytes, and names may not contain backslash, colon or ASCII control
bytes. Duplicate names are rejected by a second bounded scan. At most 128
non-trailer entries are accepted. An empty `TRAILER!!!` entry is mandatory and
only zero bytes may follow it.

All data access is through checked slices and `get`; the only subtraction
based on name length follows the prior `name_size >= 1` check. `align4` uses
checked addition. The quadratic duplicate scan has a fixed upper bound of 128
entries. Malformed input yields `CpioError`; the `Archive` fields are private,
so the provider only receives validated archive state. No archive-controlled
heap allocation occurs. Parser tests cover valid entries, truncated headers,
bad magic/hex, name/data bounds, NUL and padding errors, missing trailer,
unsupported types, duplicates and noncanonical/overlong names.

## Volume path and registry

`VfsPath` uses a fixed 255-byte path buffer, fixed arrays for at most 124
components, a 16-byte volume-name bound and a 63-byte component bound. Volume
names start with an ASCII letter and continue with ASCII letters, digits,
`_`, or `-`; matching and duplicate detection are ASCII case-insensitive.
Components are UTF-8, case-preserving, and never lowercased, uppercased,
Unicode-folded or otherwise case-normalized by generic parsing. The provider
owns matching semantics; Boot:/CPIO compares exact component bytes and is
case-sensitive. Host tests verify `Boot:`, `boot:` and `BOOT:` select the same
volume, exact spelling is delivered to a test provider, and differently cased
CPIO components do not resolve.

The grammar requires `VolumeName:` followed by a root separator. Canonical
serialization uses backslash; slash is accepted as an alias. Repeated and
trailing separators collapse, `.` is ignored, and `..` removes a component but
cannot escape the volume root. The audit-driven fix ensures terminal `..`
serialization also collapses its preceding separator. NUL, ASCII controls and
colon within components are rejected; empty volume, relative volume paths,
UNC/device syntax and overlength paths/components are rejected. No parsing
allocation occurs. UTF-8 slicing happens only at the ASCII colon separator
and whole component boundaries.

The registry is a fixed array of eight entries. It rejects invalid names,
case-insensitive duplicates and capacity exhaustion with typed errors. The
kernel publishes one static `Boot:` provider and one `Vfs` through `spin::Once`
during single-threaded boot, before interrupts are enabled. Provider and
ramdisk lifetimes are `'static`; after publication there is no mutation API.
`Once::get` and read-only provider operations require no filesystem lock, so
there is no VFS lock to recurse into from PIT IRQ and no filesystem lock to
cross a scheduler handoff, CR3 activation or CPL3 entry. Filesystem parsing and
lookup are invoked only by boot initialization and `spawn_path`, never by the
timer scheduler. Future mutable/hot-plug volumes must introduce a reviewed
IRQ-safe locking and lock-order policy first.

## VFS/provider boundary and archive-backed views

The `kernel/fs-core` VFS knows volumes, paths, node kind, provider handles and
bounded reads; it does not parse CPIO or ELF and does not create processes.
`FilesystemProvider` receives a bounded slice of validated, case-preserving
components. It defines component matching and returns regular/directory
metadata. InitrdFs alone translates those components to CPIO slash names and
owns archive offsets. Its `lookup` creates opaque offset handles after an
exact archive lookup; `read` rechecks offset/range and copies no more than the
validated file remainder. `Node::read` rejects directories and offsets beyond
EOF, clamps the output buffer to the metadata length, rejects provider counts
larger than that bound, and returns zero at EOF. Ancestor lookup enforces
`NotFound` and `NotDirectory`; directory reads return `IsDirectory`; an
unregistered volume returns `VolumeNotFound`.

The optional `contiguous_file` hook returns an immutable view only when a
provider can safely supply one. `Node::contiguous_bytes` checks the returned
length against metadata. It is explicitly optional and is not required of
future disk providers. For CPIO, the view borrows validated archive-backed
bytes and avoids copying the init image.

The borrow chain is `BootInfo mapping -> &'static [u8] -> Archive -> InitrdFs
-> VFS Node -> spawn_path -> spawn_elf -> ELF loader`. All references are
bounded by the retained kernel-lifetime ramdisk mapping. `load_static_elf`
validates the source first, copies each validated PT_LOAD file range into
process-owned user pages, finalizes permissions and returns only entry/page
metadata. The task and running process do not retain the source slice after
construction.

## Process construction, publication and PID 1

The release `spawn_elf` path reserves a generation-bearing `ProcessId` and
monotonic numeric PID, leaving the slot Creating. It then uses the architecture
memory layer under its kernel-address-space guard to create an isolated root,
map the guarded user stack, call the existing ELF validator/loader and validate
the entry/user-stack context. It attaches the completed address space to the
Creating process, prepares a task that is not queued, then records its primary
task. Any returned construction error cancels an unqueued Prepared task if
one exists and aborts Creating; abort invalidates the `ProcessId` and drops the
owned address-space object. Physical frames remain monotonic, as in 0.0.9.

Final publication disables interrupts and locks scheduler before process
table. It validates the task generation/state/type, queue capacity and
Creating process/task/address-space tuple; then marks the process Alive,
pushes the task into the ready queue under the same scheduler lock and marks it
Ready/queued. The queue push after the locked capacity check is an internal
invariant and uses `expect`; failure would indicate kernel corruption and
panic, not hostile input. No fallible operation follows the Alive transition.
This preserves the existing invariant that a timer cannot run a partially
constructed process or a task whose process is not Alive. Address-space
ownership, lower-half isolation, CR3 policy, per-task kernel stack and TSS.RSP0
selection remain in their existing architecture/scheduler layers.

The production boot path creates no userspace process before init. Process
model self-tests use a local model, not the global PID allocator; other boot
smoke tasks are kernel tasks. The first successful reservation is therefore
PID 1, with `ProcessId` still a distinct slot/generation identity. The
parentless `spawn_path(None, "Boot:\\pvOS\\System\\Init")` names the process
from its final component (`Init`). A failed reservation may consume a
monotonic PID, but every init construction failure is boot-fatal and normal
boot does not retry, so init cannot silently appear as PID 2. Main snapshots
the published process and asserts numeric PID 1 before proceeding. QEMU logs
also identify it as PID 1, then show the CPL3 marker.

## ELF policy and userspace init

`spawn_path` only resolves a regular file and passes its immutable bytes to
`spawn_elf`. Neither VFS nor CPIO bypasses the existing ELF64 policy. The
loader checks ELF64, little endian, x86_64, current ELF version and `ET_EXEC`;
rejects PT_INTERP, PT_DYNAMIC and PT_TLS; bounds the header/program-header
table and load file ranges; enforces canonical/user-address limits, alignment,
segment non-overlap, segment/page limits, the reserved stack/guard exclusion,
and an entry inside an executable load segment. It rejects W+X, zeroes mapped
pages before copying file bytes, retains zero-filled BSS, and applies final
read-only, RW/NX or executable/non-writable user permissions. The existing
guarded stack and context validation are used unchanged. The malformed-ELF
and W+X QEMU scenarios reject through this same loader.

`userspace/init` is a separate `x86_64-unknown-none` workspace artifact with
no Rust crate dependencies or libc. Its linker script fixes an ET_EXEC base;
the build validator confirms the resulting image is static and checks
interpreter/dynamic/TLS/W+X constraints. `_start` aligns the stack then calls
the minimal Rust body. The body invokes `getpid`, emits a failure/exit path if
the result is not 1, emits `I` on success and remains runnable. It uses scalar
syscall arguments and does not request floating-point/SIMD state. ELF is only
the internal bootstrap representation; no final pvOS application format is
claimed.

Under the dedicated `initrd-boot-test` feature only, the syscall dispatcher
accepts that `I` marker when the syscall entry was validated as CPL3 and
`current_user_pid() == Some(1)`, logs the accepted marker and writes QEMU's
test-success port. This occurs only after the image has passed ELF validation,
the task/process pair was published and the CPU entered the user context. An
arbitrary non-PID-1 task cannot satisfy the check, and kernel code cannot
directly call the dispatcher through the live syscall frame contract. The
normal init has no test-feature exit code and remains runnable; the feature is
not enabled in normal/release builds. This is direct evidence that PID 1 ran
at CPL3 and executed its marker syscall, not evidence of broader service
supervision or ABI behavior.

## Boot failures, locks, unsafe code and resource bounds

Missing ramdisk, invalid metadata, invalid CPIO, missing `Boot:`, failed
registration, absent init, non-regular init, malformed ELF/W+X rejection, and
process/task construction failure are deterministic boot-fatal outcomes for a
normal image. Test-only scenarios turn only their selected expected failure
into QEMU success; unexpected success exits failure. The panic and allocation
error handlers remain fatal. New `expect`/assert sites are either validated
internal states (Once publication, parsed canonical UTF-8, bounded component
index, published PID-1 snapshot) or the existing queue invariant after its
capacity and lock checks. Hostile CPIO/path data follows checked errors and
does not reach a panic or unchecked pointer construction.

The new unsafe slice construction in `BootRamdisk::from_mapping` is preceded
by scalar validation and relies on the pinned bootloader's mapped-address
contract plus completed address-space mapping audit. No unsafe code exists in
the generic path or VFS parser. Existing task-context, scheduler-stack and
ELF mapping unsafe code remains governed by the audited 0.0.9 invariants; the
initrd dataflow does not provide it with pointers or mutable data. The scalar
user syscall assembly in init uses the documented `int 0x80` ABI and states
its GPR/argument constraints in its safety comment.

Bounds remain explicit: 8 VFS volumes, 255 path/archive-name bytes, 16-byte
volume names, 63-byte components, 124 components, 128 CPIO entries, 16 ELF
load segments, 4096 ELF pages, 8 process slots and the established fixed task
capacity. Generic VFS lookup uses stack arrays and no heap allocation; archive
scanning is bounded by the entry cap; init image pages are capped by the ELF
loader. Total ramdisk bytes are bounded by the bootloader-provided mapped
memory region and checked before slice creation.

No process-table lock is held during VFS lookup, archive parsing, or ELF
validation. Memory-space mutation is guarded by the architecture memory lock;
process metadata locks are short and IRQ-masked. The only combined final
publication order is scheduler then process table with IF clear. No filesystem
lock exists; initrd/VFS work is outside IRQ/scheduler paths, CR3 switches and
userspace entry.

## Build, release artifacts and CI

Local `cargo build`/`cargo run` and release packaging compile the same root
build script. The archive builder writes stable entry order (`pvOS`,
`pvOS/System`, `pvOS/System/Init`), fixed inode numbers, zero UID/GID/time and
fixed modes/link counts; it uses no external CPIO command or host path. The
archive is passed through pinned `UefiBoot::set_ramdisk` before image creation.
The release action consumes Cargo's reported output directory, validates the
archive and init ELF, and packages the kernel ELF, raw initrd and bootable UEFI
image, then generates SPDX SBOM, manifest and SHA-256 checksums. The validator
requires the canonical `pvOS/System/Init` path and static x86_64 ET_EXEC
constraints; it rejects legacy `bin/init` as a substitute.

CI adds the positive initrd boot and seven controlled rejection scenarios:
missing ramdisk, corrupt CPIO, missing init, malformed ELF, W+X ELF, missing
volume and duplicate registration. Each is a distinct feature build; the
positive path is checked by the PID-1 CPL3 marker. The composite QEMU action
requires the runner's exact `isa-debug-exit` success status, so a kernel panic,
unrelated failure, timeout, or unexpected acceptance does not count as a
pass. Existing CI matrix entries for memory, scheduler, user-copy, process,
address-space, ELF, syscall and fault scenarios remain. The local runtime
matrix below additionally records the complete 34-scenario run.

## Runtime and validation evidence

The host had QEMU at `C:\\Program Files\\qemu\\qemu-system-x86_64.exe`; it was
added to `PATH` for the runner. The complete matrix was rerun sequentially
against the final audited tree after the path canonicalization correction.
All **34 / 34 QEMU scenarios passed on the final audited tree**: 8 new
initrd/VFS/init scenarios plus 26 established 0.0.9 regression scenarios.

The positive runtime log sequence was:

```text
[initrd] ramdisk detected
[initrd] archive validated
[vfs] registering initrd filesystem as Boot:
[vfs] boot volume ready
[init] resolving Boot:\\pvOS\\System\\Init
[init] loading boot init
[process] init created with PID 1
[init] boot init runnable
...
[init] entered CPL3
[syscall] debug byte from CPL3: 0x49
[initrd-test] PID 1 CPL3 marker accepted
```

This demonstrates the image-to-BootInfo transport, checked archive parse,
volume registration/lookup, existing ELF load, process publication, numeric
PID 1 and actual CPL3 user syscall. It does not establish persistent-volume
discovery, behavior on physical hardware beyond QEMU, multicore safety, or a
stable ABI.

Final local validation on the audited tree:

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | PASS |
| `cargo test --workspace --locked` | PASS; 15 host filesystem tests |
| `cargo build --locked` | PASS |
| `cargo build --release --locked` | PASS |
| `cargo clippy -p kernel --locked --target x86_64-unknown-none -- -D warnings` | PASS |
| `cargo clippy -p pvkernel-runner --locked -- -D warnings` | PASS |
| `cargo audit --deny warnings --ignore RUSTSEC-2025-0141` | PASS; documented existing `bincode` advisory exception only |
| `cargo deny check` | PASS; existing unmatched-license and duplicate-version warnings only |
| `actionlint` on all `.github/workflows/*.yml` | PASS |
| `python -I scripts/check_security_policy.py .` | PASS |
| `python -I -m unittest discover -s scripts/tests` | PASS; 66 tests on Windows after fixture portability fix |
| `scripts/validate_initrd.py` on release-profile output | PASS; 3 entries, static x86_64 ET_EXEC init (4,872 bytes) |
| QEMU complete matrix | PASS; 34 / 34 scenarios passed on the final audited tree |
| `git diff --check` | PASS |

The release-profile UEFI image used the same build-script path that invokes
`set_ramdisk`; its initrd validator passed, and the dedicated release-profile
QEMU run consumed that image and reached the PID 1 CPL3 marker. On the final
release build, `pvkernel-kernel.elf` was 691,704 bytes,
`pvkernel-initrd.cpio` was 5,364 bytes, and `pvkernel-uefi.img` was 2,162,688
bytes. These checks do not constitute a published release or GitHub Actions
run.

## Accepted limitations

* `Boot:` is temporary, read-only CPIO-backed boot storage; there is no
  persistent `C:` volume or disk/block driver.
* No NTFS, exFAT, FAT32, pvFS, partition discovery or volume assignment exists.
* There are no user file descriptors or general filesystem syscalls.
* There is no general `exec`, `fork`, dynamic linker, shell, or persistent
  storage.
* Static ELF64 `ET_EXEC` is an internal bootstrap representation. PVX, PVAPP,
  PVPKG, application bundles, signing and package management are future
  direction only.
* There is no stable userspace ABI; PID 1 does not yet implement general
  service supervision/reaping semantics.
* The initrd mapping remains supervisor RW/NX and relies on the current
  kernel's read-only software use.

These are documented scope limits, not release blockers for 0.0.10.

## Verdict

**READY FOR RC1 REVIEW.** No P0, P1 or P2 findings remain. The three concrete
P3 defects found in the review were fixed and revalidated. The remaining P3
item is the documented RW/NX boot mapping permission, with no current kernel
write path and no user-accessible mapping. The full 34-scenario QEMU matrix
passed on the final audited tree, and the release-profile initrd boot path was
exercised. This audit does not create an RC tag, release, commit, or push;
those remain deliberate follow-up actions.
