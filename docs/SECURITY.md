# Security model — WASMNux

## Trust boundaries

1. **The guest is fully untrusted.** Arbitrary RISC-V code (a kernel and
   its userspace) executes inside the Rust emulator. The guest can only
   touch: its own RAM, the virtual UART, the virtual block device, the
   framebuffer region, and (future) the virtual network device. All guest
   physical addresses are bounds-checked in `memory.rs`/`bus.rs`; unmapped
   accesses become guest access-fault traps, never host reads.
2. **WASM sandbox inside the browser sandbox.** The emulator itself runs
   as WebAssembly in a dedicated Worker. Even a hypothetical emulator bug
   that left guest-visible memory cannot address host JS objects: the only
   crossings are the explicit `wasm-bindgen` functions (byte arrays in/out).
3. **The guest has no host syscall surface.** No host filesystem, no host
   network sockets, no DOM, no clipboard without a user gesture-driven
   terminal paste. Persistent storage holds only the guest's own disk
   bytes under origin-scoped OPFS/IndexedDB.

## Input validation at every boundary

- **Worker protocol** (`web/src/vm/protocol.ts`): both directions are
  runtime-validated against a closed set of message shapes. Malformed
  payloads are rejected with an error event, including hostile profile ids
  and boot modes.
- **Storage keys** (`chunkStore.assertSafeKey`): slash paths only, no
  `..`, no absolute paths, no backslashes. Keys are derived from validated
  profile ids and internal counters — never from guest-controlled strings.
- **Disk import** (UI): size must be a positive multiple of 512 and
  ≤ 4 GiB; snapshots/profiles are namespaced so one profile's import
  cannot address another's chunks. Chunk CRC-32s are verified on every
  read; corruption is a loud error, never silent zero-fill.
- **Guest manifests** (`parseGuestManifest`): filenames must be bare names
  (no paths), so a compromised or malformed manifest cannot redirect
  fetches outside the profile's asset directory.
- **ELF loading** (`elf.rs`): every offset/size is bounds-checked
  (class/machine/segments), so malformed firmware images fail closed.
- **VirtIO descriptor chains** (`virtio_blk.rs`): guest memory is never
  trusted — chain loops, bad indices and wrong-direction descriptors
  complete the request with an I/O error instead of panicking, and ring
  walks are length-capped.
- **No dynamic code execution** anywhere in the app: no `eval`, no
  `Function`, no remotely-fetched scripts. The only fetched binaries are
  the WASM module and guest images from the app's own origin.

## Storage safety

- Chunk objects are immutable and the disk manifest flip in `flush()` is
  the single commit point; an interrupted tab leaves the previous,
  CRC-consistent generation.
- Persistence is never *claimed*: the UI reads
  `navigator.storage.persisted()/estimate()` and displays the real state,
  including a warning when running on the in-memory fallback backend.
- Disk export exists precisely because browser storage is evictable;
  backups are the user's ultimate guarantee and the docs say so.

## Dependency posture

- Rust core: **one** third-party crate (`wasm-bindgen`), in the bindings
  crate only; the emulator itself is dependency-free. Locked via
  `Cargo.lock` (present after the first CI run that generates it; runs use
  the committed lock).
- Web: `xterm.js` (terminal), Vite/TS/Vitest/Playwright (tooling).
  Versions pinned by `package-lock.json`; `npm audit`/`cargo audit` are
  part of the Phase-10 checklist before release.
- Guests are *aggregated*, never linked: kernel/BusyBox GPLv2, OpenSBI
  BSD-2-Clause, Debian per DFSG — see `docs/DEPENDENCIES.md`.

## Known limits (explicit, tracked)

- No attestation of guest asset integrity beyond transport + kernel
  sha256 verification at build time; a deployment that serves malicious
  guest images serves a malicious *guest* (still sandboxed as above).
- The future network relay (Phase 9) introduces a server-side component
  with its own abuse profile; it must be rate-limited and destination-
  filtered before use, and is off by default — the guest is offline-capable.
- Timing side-channels between guest and host are inherent to an
  interpreter in a shared process and out of scope for this threat model.
