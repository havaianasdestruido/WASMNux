//! WebAssembly runtime boundary (`wasm32-unknown-unknown`) for WASMNux.
//!
//! Exposes a zero-`unsafe` C ABI (`wasmnux_*`) over Rust-managed linear-memory staging buffers
//! and direct zero-copy `SimpleFB` VRAM views.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Mutex, Once};

use wasmnux_net::{BridgeHttpRequest, BridgeHttpResponse};
use wasmnux_riscv::hart::StepOutcome;
use wasmnux_riscv::trap::SystemResetAction;
use wasmnux_riscv::{RiscvMachine, VmLifecycleState};
use wasmnux_storage::{validate_identifier, DirtyChunkRecord, SparseBlockDisk, CHUNK_SIZE};
use wasmnux_virtio::{VirtioBlkMmio, VirtioNetMmio, VirtioRngMmio};

/// ABI version used by TypeScript bindings to verify compatibility at initialization.
pub const WASMNUX_ABI_VERSION: u32 = 1;

pub const VIRTIO_BLK_BASE: u64 = 0x1000_1000;
pub const VIRTIO_NET_BASE: u64 = 0x1000_2000;
pub const VIRTIO_RNG_BASE: u64 = 0x1000_3000;

pub const MIN_GUEST_RAM_MIB: u32 = 16;
pub const MAX_GUEST_RAM_MIB: u32 = 1024;
pub const MIN_DISK_MIB: u32 = 1;
pub const MAX_DISK_MIB: u32 = 2048;
pub const MAX_IO_BUF_BYTES: usize = 256 * 1024 * 1024;

const FULL_SNAP_MAGIC: [u8; 8] = *b"WNUXFULL";
const FULL_SNAP_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum WasmErrorCode {
    Ok = 0,
    InvalidHandle = 1,
    OutOfBounds = 2,
    InvalidArgument = 3,
    StorageError = 4,
    BootError = 5,
    SnapshotError = 6,
    LifecycleError = 7,
}

struct VmEntry {
    profile_id: String,
    machine: RiscvMachine,
    staged_kernel: Vec<u8>,
    staged_initrd: Vec<u8>,
    last_bootargs: String,
    pending_dirty: VecDeque<DirtyChunkRecord>,
    last_dirty_crc32: u32,
    pending_http: VecDeque<BridgeHttpRequest>,
    total_steps: u64,
}

struct RuntimeRegistry {
    next_handle: u32,
    vms: BTreeMap<u32, VmEntry>,
    io_buf: Vec<u8>,
    last_error_code: WasmErrorCode,
    last_error_msg: String,
}

impl RuntimeRegistry {
    const fn new() -> Self {
        Self {
            next_handle: 1,
            vms: BTreeMap::new(),
            io_buf: Vec::new(),
            last_error_code: WasmErrorCode::Ok,
            last_error_msg: String::new(),
        }
    }

    fn clear_err(&mut self) {
        self.last_error_code = WasmErrorCode::Ok;
        self.last_error_msg.clear();
    }

    fn set_err(&mut self, code: WasmErrorCode, msg: impl Into<String>) {
        self.last_error_code = code;
        self.last_error_msg = msg.into();
    }

    fn write_io_output(&mut self, bytes: &[u8]) -> usize {
        if bytes.len() > MAX_IO_BUF_BYTES {
            self.set_err(
                WasmErrorCode::OutOfBounds,
                "output exceeds MAX_IO_BUF_BYTES",
            );
            return 0;
        }
        self.io_buf.clear();
        self.io_buf.extend_from_slice(bytes);
        self.io_buf.len()
    }
}

static INIT_HOOK: Once = Once::new();
static RUNTIME: Mutex<RuntimeRegistry> = Mutex::new(RuntimeRegistry::new());

fn install_panic_hook() {
    INIT_HOOK.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            if let Ok(mut rt) = RUNTIME.lock() {
                rt.set_err(
                    WasmErrorCode::InvalidArgument,
                    format!("WASM runtime panic trapped: {info}"),
                );
            }
        }));
    });
}

fn with_rt<R>(f: impl FnOnce(&mut RuntimeRegistry) -> R) -> R {
    install_panic_hook();
    let mut guard = match RUNTIME.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    f(&mut guard)
}

fn with_blk_mut<R>(
    machine: &mut RiscvMachine,
    f: impl FnOnce(&mut VirtioBlkMmio) -> R,
) -> Option<R> {
    machine
        .bus
        .with_mmio_mut(VIRTIO_BLK_BASE, |dev, _ram, _plic| {
            let blk = dev.as_any_mut().downcast_mut::<VirtioBlkMmio>()?;
            Some(f(blk))
        })?
}

fn with_blk<R>(machine: &RiscvMachine, f: impl FnOnce(&VirtioBlkMmio) -> R) -> Option<R> {
    machine.bus.with_mmio(VIRTIO_BLK_BASE, |dev| {
        let blk = dev.as_any().downcast_ref::<VirtioBlkMmio>()?;
        Some(f(blk))
    })?
}

fn encode_http_request(req: &BridgeHttpRequest) -> Vec<u8> {
    let mut out = Vec::with_capacity(256 + req.body.len());
    out.extend_from_slice(&req.request_id.to_le_bytes());
    let put_str = |buf: &mut Vec<u8>, s: &str| {
        let b = s.as_bytes();
        buf.extend_from_slice(&(b.len() as u32).to_le_bytes());
        buf.extend_from_slice(b);
    };
    put_str(&mut out, &req.method);
    put_str(&mut out, &req.host);
    put_str(&mut out, &req.path);
    out.extend_from_slice(&(req.headers.len() as u32).to_le_bytes());
    for (k, v) in &req.headers {
        put_str(&mut out, k);
        put_str(&mut out, v);
    }
    out.extend_from_slice(&(req.body.len() as u32).to_le_bytes());
    out.extend_from_slice(&req.body);
    out
}

fn decode_http_response(raw: &[u8]) -> Result<BridgeHttpResponse, &'static str> {
    let take_u32 = |c: &mut usize| -> Result<u32, &'static str> {
        let s = raw.get(*c..*c + 4).ok_or("truncated HTTP response u32")?;
        *c += 4;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    };
    let take_str = |c: &mut usize| -> Result<String, &'static str> {
        let len = take_u32(c)? as usize;
        let s = raw
            .get(*c..*c + len)
            .ok_or("truncated HTTP response string")?;
        *c += len;
        std::str::from_utf8(s)
            .map(str::to_owned)
            .map_err(|_| "invalid UTF-8 in HTTP response")
    };
    let req_s = raw.get(0..10).ok_or("HTTP response shorter than header")?;
    let request_id = u64::from_le_bytes([
        req_s[0], req_s[1], req_s[2], req_s[3], req_s[4], req_s[5], req_s[6], req_s[7],
    ]);
    let status_code = u16::from_le_bytes([req_s[8], req_s[9]]);
    let mut cur = 10usize;
    let status_text = take_str(&mut cur)?;
    let hdr_count = take_u32(&mut cur)? as usize;
    if hdr_count > 256 {
        return Err("excessive HTTP response header count");
    }
    let mut headers = Vec::with_capacity(hdr_count);
    for _ in 0..hdr_count {
        let k = take_str(&mut cur)?;
        let v = take_str(&mut cur)?;
        headers.push((k, v));
    }
    let body_len = take_u32(&mut cur)? as usize;
    let body = raw
        .get(cur..cur + body_len)
        .ok_or("truncated HTTP response body")?
        .to_vec();
    Ok(BridgeHttpResponse {
        request_id,
        status_code,
        status_text,
        headers,
        body,
    })
}

/// Return the compiled WASMNux WASM runtime ABI version.
#[no_mangle]
pub extern "C" fn wasmnux_abi_version() -> u32 {
    WASMNUX_ABI_VERSION
}

/// Resize the shared Rust-owned I/O staging buffer to `len` bytes and return its linear memory offset (`0` on error).
#[no_mangle]
pub extern "C" fn wasmnux_io_buf_resize(len: usize) -> usize {
    with_rt(|rt| {
        if len > MAX_IO_BUF_BYTES {
            rt.set_err(
                WasmErrorCode::OutOfBounds,
                format!("requested I/O buffer size {len} exceeds ceiling {MAX_IO_BUF_BYTES}"),
            );
            return 0;
        }
        rt.clear_err();
        rt.io_buf.resize(len.max(1), 0);
        rt.io_buf.as_mut_ptr() as usize
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_io_buf_ptr() -> usize {
    with_rt(|rt| {
        if rt.io_buf.is_empty() {
            rt.io_buf.resize(1, 0);
        }
        rt.io_buf.as_ptr() as usize
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_io_buf_len() -> usize {
    with_rt(|rt| rt.io_buf.len())
}

#[no_mangle]
pub extern "C" fn wasmnux_last_error_code() -> u32 {
    with_rt(|rt| rt.last_error_code as u32)
}

#[no_mangle]
pub extern "C" fn wasmnux_last_error_read() -> usize {
    with_rt(|rt| {
        let msg = rt.last_error_msg.clone();
        rt.io_buf.clear();
        rt.io_buf.extend_from_slice(msg.as_bytes());
        rt.io_buf.len()
    })
}

/// Create a new isolated VM instance for the profile identifier currently in `io_buf[..profile_id_len]`.
#[no_mangle]
pub extern "C" fn wasmnux_vm_create(
    ram_mib: u32,
    disk_mib: u32,
    epoch_sec_lo: u32,
    epoch_sec_hi: u32,
    profile_id_len: usize,
) -> u32 {
    with_rt(|rt| {
        if !(MIN_GUEST_RAM_MIB..=MAX_GUEST_RAM_MIB).contains(&ram_mib) {
            rt.set_err(
                WasmErrorCode::InvalidArgument,
                format!("guest RAM {ram_mib} MiB out of bounds [{MIN_GUEST_RAM_MIB}, {MAX_GUEST_RAM_MIB}]"),
            );
            return 0;
        }
        if !(MIN_DISK_MIB..=MAX_DISK_MIB).contains(&disk_mib) {
            rt.set_err(
                WasmErrorCode::InvalidArgument,
                format!(
                    "disk capacity {disk_mib} MiB out of bounds [{MIN_DISK_MIB}, {MAX_DISK_MIB}]"
                ),
            );
            return 0;
        }
        let Some(id_slice) = rt.io_buf.get(..profile_id_len) else {
            rt.set_err(WasmErrorCode::OutOfBounds, "profile_id_len exceeds io_buf");
            return 0;
        };
        let Ok(profile_id) = std::str::from_utf8(id_slice).map(str::to_owned) else {
            rt.set_err(
                WasmErrorCode::InvalidArgument,
                "profile_id is not valid UTF-8",
            );
            return 0;
        };
        if let Err(e) = validate_identifier(&profile_id) {
            rt.set_err(WasmErrorCode::InvalidArgument, e.to_string());
            return 0;
        }

        let Some(ram_bytes) = (ram_mib as usize).checked_mul(1024 * 1024) else {
            rt.set_err(WasmErrorCode::OutOfBounds, "guest RAM byte size overflow");
            return 0;
        };
        let disk_bytes = u64::from(disk_mib) * 1024 * 1024;
        let disk = match SparseBlockDisk::new(disk_bytes) {
            Ok(d) => d,
            Err(e) => {
                rt.set_err(WasmErrorCode::StorageError, e.to_string());
                return 0;
            }
        };

        let epoch_secs = (u64::from(epoch_sec_hi) << 32) | u64::from(epoch_sec_lo);
        let base_epoch_ns = epoch_secs.saturating_mul(1_000_000_000);
        let mut machine = RiscvMachine::new(ram_bytes, base_epoch_ns);

        // Derive deterministic-yet-distinct VirtIO-RNG seed from epoch + profile_id
        let mut rng_seed = [0x5Au8; 16];
        for (i, &b) in profile_id.as_bytes().iter().enumerate() {
            rng_seed[i % 16] ^= b.wrapping_add(i as u8);
        }
        let ep_bytes = base_epoch_ns.to_le_bytes();
        for i in 0..8 {
            rng_seed[i] ^= ep_bytes[i];
        }

        machine.attach_virtio_mmio(
            VIRTIO_BLK_BASE,
            0x1000,
            1,
            Box::new(VirtioBlkMmio::new(1, disk)),
        );
        machine.attach_virtio_mmio(
            VIRTIO_NET_BASE,
            0x1000,
            2,
            Box::new(VirtioNetMmio::new_default(2)),
        );
        machine.attach_virtio_mmio(
            VIRTIO_RNG_BASE,
            0x1000,
            3,
            Box::new(VirtioRngMmio::new(3, rng_seed)),
        );

        let handle = rt.next_handle;
        rt.next_handle = rt.next_handle.wrapping_add(1).max(1);
        rt.vms.insert(
            handle,
            VmEntry {
                profile_id,
                machine,
                staged_kernel: Vec::new(),
                staged_initrd: Vec::new(),
                last_bootargs: String::new(),
                pending_dirty: VecDeque::new(),
                last_dirty_crc32: 0,
                pending_http: VecDeque::new(),
                total_steps: 0,
            },
        );
        rt.clear_err();
        handle
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_vm_destroy(handle: u32) -> i32 {
    with_rt(|rt| {
        if rt.vms.remove(&handle).is_some() {
            rt.clear_err();
            0
        } else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            -1
        }
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_vm_stage_kernel(handle: u32, len: usize) -> i32 {
    with_rt(|rt| {
        let Some(bytes) = rt.io_buf.get(..len).map(<[u8]>::to_vec) else {
            rt.set_err(WasmErrorCode::OutOfBounds, "kernel length exceeds io_buf");
            return -1;
        };
        let Some(vm) = rt.vms.get_mut(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return -1;
        };
        vm.staged_kernel = bytes;
        rt.clear_err();
        0
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_vm_stage_initrd(handle: u32, len: usize) -> i32 {
    with_rt(|rt| {
        let Some(bytes) = rt.io_buf.get(..len).map(<[u8]>::to_vec) else {
            rt.set_err(WasmErrorCode::OutOfBounds, "initrd length exceeds io_buf");
            return -1;
        };
        let Some(vm) = rt.vms.get_mut(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return -1;
        };
        vm.staged_initrd = bytes;
        rt.clear_err();
        0
    })
}

/// Boot the VM using previously staged kernel (+ optional initrd) and `bootargs` in `io_buf[..bootargs_len]`.
#[no_mangle]
pub extern "C" fn wasmnux_vm_boot(handle: u32, bootargs_len: usize) -> i32 {
    with_rt(|rt| {
        let Some(arg_slice) = rt.io_buf.get(..bootargs_len) else {
            rt.set_err(WasmErrorCode::OutOfBounds, "bootargs_len exceeds io_buf");
            return -1;
        };
        let Ok(bootargs) = std::str::from_utf8(arg_slice).map(str::to_owned) else {
            rt.set_err(
                WasmErrorCode::InvalidArgument,
                "bootargs must be valid UTF-8",
            );
            return -1;
        };
        let Some(vm) = rt.vms.get_mut(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return -1;
        };
        let mut seed = [0xA5u8; 32];
        for (i, &b) in vm.profile_id.as_bytes().iter().enumerate() {
            seed[i % 32] ^= b;
        }
        let initrd_opt = if vm.staged_initrd.is_empty() {
            None
        } else {
            Some(vm.staged_initrd.as_slice())
        };
        if let Err(msg) =
            vm.machine
                .load_linux_image(&vm.staged_kernel, initrd_opt, &bootargs, seed)
        {
            rt.set_err(WasmErrorCode::BootError, msg);
            return -1;
        }
        vm.last_bootargs = bootargs;
        rt.clear_err();
        0
    })
}

/// Execute up to `max_steps` instructions (`max 0x0FFF_FFFF`).
/// Returns `(outcome_code << 28) | (steps & 0x0FFF_FFFF)`.
#[no_mangle]
pub extern "C" fn wasmnux_vm_step(handle: u32, max_steps: u32) -> u32 {
    with_rt(|rt| {
        let Some(vm) = rt.vms.get_mut(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return 15 << 28;
        };
        let capped = (max_steps.min(0x0FFF_FFFF)) as usize;
        let (steps, outcome) = vm.machine.step_slice(capped);
        vm.total_steps = vm.total_steps.wrapping_add(steps as u64);
        let code: u32 = match outcome {
            StepOutcome::Executed => 0,
            StepOutcome::WfiIdle => 1,
            StepOutcome::Reset(SystemResetAction::PowerOff) => 2,
            StepOutcome::Reset(SystemResetAction::ColdReboot | SystemResetAction::WarmReboot) => 3,
        };
        (code << 28) | ((steps as u32) & 0x0FFF_FFFF)
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_vm_pause(handle: u32) -> i32 {
    with_rt(|rt| {
        let Some(vm) = rt.vms.get_mut(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return -1;
        };
        match vm.machine.pause() {
            Ok(()) => {
                rt.clear_err();
                0
            }
            Err(msg) => {
                rt.set_err(WasmErrorCode::LifecycleError, msg);
                -1
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_vm_resume(handle: u32) -> i32 {
    with_rt(|rt| {
        let Some(vm) = rt.vms.get_mut(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return -1;
        };
        match vm.machine.resume() {
            Ok(()) => {
                rt.clear_err();
                0
            }
            Err(msg) => {
                rt.set_err(WasmErrorCode::LifecycleError, msg);
                -1
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_vm_hard_reset(handle: u32) -> i32 {
    with_rt(|rt| {
        let Some(vm) = rt.vms.get_mut(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return -1;
        };
        if !vm.staged_kernel.is_empty() {
            // Reset guest DRAM, interrupt controllers, timers, and VirtIO MMIO rings
            // while preserving VirtIO-Blk SparseBlockDisk data across warm/cold reboots!
            vm.machine.bus.ram.as_mut_slice().fill(0);
            vm.machine.bus.clint.mtime = 0;
            vm.machine.bus.clint.mtimecmp = u64::MAX;
            vm.machine.bus.clint.msip = 0;
            vm.machine.bus.syscon.reset_action = None;
            vm.machine.bus.plic = wasmnux_riscv::bus::PlicState::new();
            vm.machine
                .bus
                .with_mmio_mut(VIRTIO_BLK_BASE, |dev, _ram, plic| {
                    if let Some(blk) = dev.as_any_mut().downcast_mut::<VirtioBlkMmio>() {
                        blk.mmio.reset(plic);
                    }
                });
            vm.machine
                .bus
                .with_mmio_mut(VIRTIO_NET_BASE, |dev, _ram, plic| {
                    if let Some(net) = dev.as_any_mut().downcast_mut::<VirtioNetMmio>() {
                        net.mmio.reset(plic);
                    }
                });
            vm.machine
                .bus
                .with_mmio_mut(VIRTIO_RNG_BASE, |dev, _ram, plic| {
                    if let Some(rng) = dev.as_any_mut().downcast_mut::<VirtioRngMmio>() {
                        rng.mmio.reset(plic);
                    }
                });

            let mut seed = [0x5Au8; 32];
            for (i, &b) in vm.profile_id.as_bytes().iter().enumerate() {
                seed[i % 32] ^= b;
            }
            let initrd_opt = if vm.staged_initrd.is_empty() {
                None
            } else {
                Some(vm.staged_initrd.as_slice())
            };
            if let Err(msg) =
                vm.machine
                    .load_linux_image(&vm.staged_kernel, initrd_opt, &vm.last_bootargs, seed)
            {
                rt.set_err(WasmErrorCode::BootError, msg);
                return -1;
            }
        } else {
            vm.machine.hard_reset();
        }
        rt.clear_err();
        0
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_vm_state(handle: u32) -> u32 {
    with_rt(|rt| {
        let Some(vm) = rt.vms.get(&handle) else {
            return u32::MAX;
        };
        match vm.machine.state {
            VmLifecycleState::Created => 0,
            VmLifecycleState::Running => 1,
            VmLifecycleState::Paused => 2,
            VmLifecycleState::PoweredOff => 3,
            VmLifecycleState::RebootRequested => 4,
        }
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_uart_push_input(handle: u32, len: usize) -> i32 {
    with_rt(|rt| {
        let Some(bytes) = rt.io_buf.get(..len).map(<[u8]>::to_vec) else {
            rt.set_err(WasmErrorCode::OutOfBounds, "UART input exceeds io_buf");
            return -1;
        };
        let Some(vm) = rt.vms.get_mut(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return -1;
        };
        vm.machine
            .bus
            .uart
            .push_input(&bytes, &mut vm.machine.bus.plic);
        vm.machine.hart.wfi = false;
        rt.clear_err();
        0
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_uart_drain_output(handle: u32) -> usize {
    with_rt(|rt| {
        let Some(vm) = rt.vms.get_mut(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return 0;
        };
        let bytes = vm.machine.bus.uart.drain_output();
        rt.clear_err();
        rt.write_io_output(&bytes)
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_fb_ptr(handle: u32) -> usize {
    with_rt(|rt| {
        rt.vms
            .get(&handle)
            .map_or(0, |vm| vm.machine.bus.fb.as_slice().as_ptr() as usize)
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_fb_byte_len(handle: u32) -> usize {
    with_rt(|rt| {
        rt.vms
            .get(&handle)
            .map_or(0, |vm| vm.machine.bus.fb.byte_len())
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_fb_width(handle: u32) -> u32 {
    with_rt(|rt| {
        rt.vms
            .get(&handle)
            .map_or(0, |vm| vm.machine.bus.fb.dimensions().0)
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_fb_height(handle: u32) -> u32 {
    with_rt(|rt| {
        rt.vms
            .get(&handle)
            .map_or(0, |vm| vm.machine.bus.fb.dimensions().1)
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_fb_resize(handle: u32, width: u32, height: u32) -> i32 {
    with_rt(|rt| {
        let Some(vm) = rt.vms.get_mut(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return -1;
        };
        vm.machine.bus.fb.resize(width, height);
        rt.clear_err();
        0
    })
}

/// Returns `(y_min << 32) | y_max` if dirty scanlines exist since last call, or `u64::MAX` if clean.
#[no_mangle]
pub extern "C" fn wasmnux_fb_take_dirty(handle: u32) -> u64 {
    with_rt(|rt| {
        let Some(vm) = rt.vms.get_mut(&handle) else {
            return u64::MAX;
        };
        match vm.machine.bus.fb.take_dirty_range() {
            Some((y0, y1)) => (u64::from(y0) << 32) | u64::from(y1),
            None => u64::MAX,
        }
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_disk_load_base_chunk(handle: u32, chunk_index: u32) -> i32 {
    with_rt(|rt| {
        let Some(chunk) = rt.io_buf.get(..CHUNK_SIZE).map(<[u8]>::to_vec) else {
            rt.set_err(
                WasmErrorCode::OutOfBounds,
                "io_buf smaller than 64 KiB chunk",
            );
            return -1;
        };
        let Some(vm) = rt.vms.get_mut(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return -1;
        };
        let res = with_blk_mut(&mut vm.machine, |blk| {
            blk.disk.load_base_chunk(chunk_index, &chunk)
        });
        match res {
            Some(Ok(())) => {
                rt.clear_err();
                0
            }
            Some(Err(e)) => {
                rt.set_err(WasmErrorCode::StorageError, e.to_string());
                -1
            }
            None => {
                rt.set_err(WasmErrorCode::StorageError, "VirtioBlk device missing");
                -1
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_disk_hydrate_overlay_chunk(
    handle: u32,
    chunk_index: u32,
    has_crc: u32,
    expected_crc32: u32,
) -> i32 {
    with_rt(|rt| {
        let Some(chunk) = rt.io_buf.get(..CHUNK_SIZE).map(<[u8]>::to_vec) else {
            rt.set_err(
                WasmErrorCode::OutOfBounds,
                "io_buf smaller than 64 KiB chunk",
            );
            return -1;
        };
        let Some(vm) = rt.vms.get_mut(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return -1;
        };
        let crc_opt = if has_crc != 0 {
            Some(expected_crc32)
        } else {
            None
        };
        let res = with_blk_mut(&mut vm.machine, |blk| {
            blk.disk.hydrate_overlay_chunk(chunk_index, &chunk, crc_opt)
        });
        match res {
            Some(Ok(())) => {
                rt.clear_err();
                0
            }
            Some(Err(e)) => {
                rt.set_err(WasmErrorCode::StorageError, e.to_string());
                -1
            }
            None => {
                rt.set_err(WasmErrorCode::StorageError, "VirtioBlk device missing");
                -1
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_disk_set_quota_chunks(handle: u32, max_chunks: u32) -> i32 {
    with_rt(|rt| {
        let Some(vm) = rt.vms.get_mut(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return -1;
        };
        let limit = if max_chunks == u32::MAX {
            None
        } else {
            Some(max_chunks as usize)
        };
        let _ = with_blk_mut(&mut vm.machine, |blk| {
            blk.disk.set_max_overlay_chunks(limit)
        });
        rt.clear_err();
        0
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_disk_set_read_only(handle: u32, read_only: u32) -> i32 {
    with_rt(|rt| {
        let Some(vm) = rt.vms.get_mut(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return -1;
        };
        let _ = with_blk_mut(&mut vm.machine, |blk| {
            blk.disk.set_read_only(read_only != 0)
        });
        rt.clear_err();
        0
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_disk_reset_overlay(handle: u32) -> i32 {
    with_rt(|rt| {
        let Some(vm) = rt.vms.get_mut(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return -1;
        };
        vm.pending_dirty.clear();
        let _ = with_blk_mut(&mut vm.machine, |blk| blk.disk.reset_overlay());
        rt.clear_err();
        0
    })
}

/// Pop next dirty 64 KiB chunk into `io_buf[..65536]`, returning `chunk_index` or `-1` when none remain.
#[no_mangle]
pub extern "C" fn wasmnux_disk_pop_dirty_chunk(handle: u32) -> i32 {
    with_rt(|rt| {
        let Some(vm) = rt.vms.get_mut(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return -1;
        };
        if vm.pending_dirty.is_empty() {
            if let Some(fresh) = with_blk_mut(&mut vm.machine, |blk| blk.disk.drain_dirty_chunks())
            {
                vm.pending_dirty.extend(fresh);
            }
        }
        let Some(rec) = vm.pending_dirty.pop_front() else {
            return -1;
        };
        vm.last_dirty_crc32 = rec.crc32;
        let idx = rec.chunk_index as i32;
        rt.clear_err();
        rt.write_io_output(&rec.data);
        idx
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_disk_last_dirty_crc32(handle: u32) -> u32 {
    with_rt(|rt| rt.vms.get(&handle).map_or(0, |vm| vm.last_dirty_crc32))
}

#[no_mangle]
pub extern "C" fn wasmnux_disk_requeue_dirty_chunk(handle: u32, chunk_index: u32) -> i32 {
    with_rt(|rt| {
        let Some(vm) = rt.vms.get_mut(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return -1;
        };
        let _ = with_blk_mut(&mut vm.machine, |blk| {
            blk.disk.requeue_dirty_chunks(&[chunk_index]);
        });
        rt.clear_err();
        0
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_disk_export_bundle(handle: u32) -> usize {
    with_rt(|rt| {
        let Some(vm) = rt.vms.get(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return 0;
        };
        let pid = vm.profile_id.clone();
        let res = with_blk(&vm.machine, |blk| blk.disk.export_overlay_bundle(&pid));
        match res {
            Some(Ok(bundle)) => {
                rt.clear_err();
                rt.write_io_output(&bundle)
            }
            Some(Err(e)) => {
                rt.set_err(WasmErrorCode::StorageError, e.to_string());
                0
            }
            None => {
                rt.set_err(WasmErrorCode::StorageError, "VirtioBlk device missing");
                0
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_disk_import_bundle(handle: u32, len: usize) -> i32 {
    with_rt(|rt| {
        let Some(bundle) = rt.io_buf.get(..len).map(<[u8]>::to_vec) else {
            rt.set_err(WasmErrorCode::OutOfBounds, "bundle length exceeds io_buf");
            return -1;
        };
        let Some(vm) = rt.vms.get_mut(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return -1;
        };
        let expected_pid = vm.profile_id.clone();
        let res = with_blk_mut(&mut vm.machine, |blk| {
            blk.disk.import_overlay_bundle(Some(&expected_pid), &bundle)
        });
        match res {
            Some(Ok(_)) => {
                vm.pending_dirty.clear();
                rt.clear_err();
                0
            }
            Some(Err(e)) => {
                rt.set_err(WasmErrorCode::StorageError, e.to_string());
                -1
            }
            None => {
                rt.set_err(WasmErrorCode::StorageError, "VirtioBlk device missing");
                -1
            }
        }
    })
}

/// Save combined CPU + Guest RAM + Disk Overlay state into `io_buf` and return total byte length.
#[no_mangle]
pub extern "C" fn wasmnux_vm_save_full_snapshot(handle: u32) -> usize {
    with_rt(|rt| {
        let Some(vm) = rt.vms.get(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return 0;
        };
        let pid = vm.profile_id.clone();
        let cpu_ram = vm.machine.save_cpu_ram_snapshot();
        let Some(Ok(disk_bundle)) =
            with_blk(&vm.machine, |blk| blk.disk.export_overlay_bundle(&pid))
        else {
            rt.set_err(
                WasmErrorCode::SnapshotError,
                "failed exporting disk overlay",
            );
            return 0;
        };
        let mut combined = Vec::with_capacity(24 + cpu_ram.len() + disk_bundle.len());
        combined.extend_from_slice(&FULL_SNAP_MAGIC);
        combined.extend_from_slice(&FULL_SNAP_VERSION.to_le_bytes());
        combined.extend_from_slice(&(cpu_ram.len() as u64).to_le_bytes());
        combined.extend_from_slice(&(disk_bundle.len() as u64).to_le_bytes());
        combined.extend_from_slice(&cpu_ram);
        combined.extend_from_slice(&disk_bundle);
        rt.clear_err();
        rt.write_io_output(&combined)
    })
}

/// Atomically validate and restore combined CPU + Guest RAM + Disk Overlay state from `io_buf[..len]`.
#[no_mangle]
pub extern "C" fn wasmnux_vm_restore_full_snapshot(handle: u32, len: usize) -> i32 {
    with_rt(|rt| {
        let Some(raw) = rt.io_buf.get(..len).map(<[u8]>::to_vec) else {
            rt.set_err(WasmErrorCode::OutOfBounds, "snapshot length exceeds io_buf");
            return -1;
        };
        if raw.len() < 28 || raw[0..8] != FULL_SNAP_MAGIC {
            rt.set_err(
                WasmErrorCode::SnapshotError,
                "invalid full VM snapshot magic",
            );
            return -1;
        }
        let ver = u32::from_le_bytes([raw[8], raw[9], raw[10], raw[11]]);
        if ver != FULL_SNAP_VERSION {
            rt.set_err(
                WasmErrorCode::SnapshotError,
                "unsupported full snapshot version",
            );
            return -1;
        }
        let cpu_len_u64 = u64::from_le_bytes([
            raw[12], raw[13], raw[14], raw[15], raw[16], raw[17], raw[18], raw[19],
        ]);
        let disk_len_u64 = u64::from_le_bytes([
            raw[20], raw[21], raw[22], raw[23], raw[24], raw[25], raw[26], raw[27],
        ]);
        let (Ok(cpu_len), Ok(disk_len)) =
            (usize::try_from(cpu_len_u64), usize::try_from(disk_len_u64))
        else {
            rt.set_err(
                WasmErrorCode::SnapshotError,
                "snapshot section size overflow",
            );
            return -1;
        };
        let Some(total_end) = 28usize
            .checked_add(cpu_len)
            .and_then(|x| x.checked_add(disk_len))
        else {
            rt.set_err(
                WasmErrorCode::SnapshotError,
                "snapshot section length overflow",
            );
            return -1;
        };
        if total_end > raw.len() {
            rt.set_err(
                WasmErrorCode::SnapshotError,
                "truncated full snapshot payload",
            );
            return -1;
        }
        let cpu_slice = &raw[28..28 + cpu_len];
        let disk_slice = &raw[28 + cpu_len..total_end];

        let Some(vm) = rt.vms.get_mut(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return -1;
        };
        let expected_pid = vm.profile_id.clone();

        // Validate disk bundle FIRST onto a temporary disk clone or before CPU commit so partial failure cannot corrupt!
        let prev_disk_snap = with_blk(&vm.machine, |blk| blk.disk.snapshot());
        let disk_import = with_blk_mut(&mut vm.machine, |blk| {
            blk.disk
                .import_overlay_bundle(Some(&expected_pid), disk_slice)
        });
        match disk_import {
            Some(Ok(_)) => {}
            Some(Err(e)) => {
                rt.set_err(WasmErrorCode::SnapshotError, e.to_string());
                return -1;
            }
            None => {
                rt.set_err(WasmErrorCode::SnapshotError, "VirtioBlk device missing");
                return -1;
            }
        }

        if let Err(msg) = vm.machine.restore_cpu_ram_snapshot(cpu_slice) {
            if let Some(snap) = prev_disk_snap {
                let _ = with_blk_mut(&mut vm.machine, |blk| blk.disk.restore_snapshot(&snap));
            }
            rt.set_err(WasmErrorCode::SnapshotError, msg);
            return -1;
        }
        vm.machine.state = VmLifecycleState::Running;
        vm.pending_dirty.clear();
        rt.clear_err();
        0
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_net_set_enabled(handle: u32, enabled: u32) -> i32 {
    with_rt(|rt| {
        let Some(vm) = rt.vms.get_mut(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return -1;
        };
        let _ = vm
            .machine
            .bus
            .with_mmio_mut(VIRTIO_NET_BASE, |dev, _ram, _plic| {
                if let Some(net) = dev.as_any_mut().downcast_mut::<VirtioNetMmio>() {
                    net.switch.set_enabled(enabled != 0);
                }
            });
        rt.clear_err();
        0
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_net_pop_http_request(handle: u32) -> usize {
    with_rt(|rt| {
        let Some(vm) = rt.vms.get_mut(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return 0;
        };
        if vm.pending_http.is_empty() {
            let drained = vm
                .machine
                .bus
                .with_mmio_mut(VIRTIO_NET_BASE, |dev, _ram, _plic| {
                    dev.as_any_mut()
                        .downcast_mut::<VirtioNetMmio>()
                        .map(VirtioNetMmio::drain_http_requests)
                })
                .flatten()
                .unwrap_or_default();
            vm.pending_http.extend(drained);
        }
        let Some(req) = vm.pending_http.pop_front() else {
            return 0;
        };
        let encoded = encode_http_request(&req);
        rt.clear_err();
        rt.write_io_output(&encoded)
    })
}

#[no_mangle]
pub extern "C" fn wasmnux_net_complete_http_response(handle: u32, len: usize) -> i32 {
    with_rt(|rt| {
        let Some(raw) = rt.io_buf.get(..len).map(<[u8]>::to_vec) else {
            rt.set_err(
                WasmErrorCode::OutOfBounds,
                "HTTP response length exceeds io_buf",
            );
            return -1;
        };
        let resp = match decode_http_response(&raw) {
            Ok(r) => r,
            Err(msg) => {
                rt.set_err(WasmErrorCode::InvalidArgument, msg);
                return -1;
            }
        };
        let Some(vm) = rt.vms.get_mut(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return -1;
        };
        vm.machine
            .bus
            .with_mmio_mut(VIRTIO_NET_BASE, |dev, ram, plic| {
                if let Some(net) = dev.as_any_mut().downcast_mut::<VirtioNetMmio>() {
                    net.deliver_http_response(resp, ram, plic);
                }
            });
        vm.machine.hart.wfi = false;
        rt.clear_err();
        0
    })
}

/// Write a 64-byte little-endian telemetry struct into `io_buf` and return `64` (or `0` on invalid handle):
/// `[0..8]` total_steps, `[8..16]` mtime, `[16..24]` ram_bytes, `[24..32]` disk_capacity_bytes,
/// `[32..40]` disk_overlay_bytes, `[40..48]` disk_bytes_read, `[48..56]` disk_bytes_written, `[56..64]` net_rx_bytes.
#[no_mangle]
pub extern "C" fn wasmnux_vm_telemetry(handle: u32) -> usize {
    with_rt(|rt| {
        let Some(vm) = rt.vms.get(&handle) else {
            rt.set_err(WasmErrorCode::InvalidHandle, "unknown VM handle");
            return 0;
        };
        let total_steps = vm.total_steps;
        let mtime = vm.machine.bus.clint.mtime;
        let ram_bytes = vm.machine.bus.ram.len() as u64;
        let (disk_cap, disk_ov, disk_r, disk_w) = with_blk(&vm.machine, |blk| {
            let (r, w, _) = blk.disk.stats();
            (blk.disk.capacity_bytes(), blk.disk.overlay_bytes(), r, w)
        })
        .unwrap_or((0, 0, 0, 0));
        let net_rx = vm
            .machine
            .bus
            .with_mmio(VIRTIO_NET_BASE, |dev| {
                dev.as_any()
                    .downcast_ref::<VirtioNetMmio>()
                    .map(|n| n.switch.stats().rx_bytes_to_guest)
            })
            .flatten()
            .unwrap_or(0);

        let mut out = [0u8; 64];
        for (idx, val) in [
            total_steps,
            mtime,
            ram_bytes,
            disk_cap,
            disk_ov,
            disk_r,
            disk_w,
            net_rx,
        ]
        .into_iter()
        .enumerate()
        {
            out[idx * 8..(idx + 1) * 8].copy_from_slice(&val.to_le_bytes());
        }
        rt.clear_err();
        rt.write_io_output(&out)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasmnux_riscv::boot::RISCV_IMAGE_MAGIC2;
    use wasmnux_riscv::KERNEL_LOAD_OFFSET;

    fn stage_bytes(data: &[u8]) {
        let ptr = wasmnux_io_buf_resize(data.len());
        assert_ne!(ptr, 0);
        with_rt(|rt| {
            rt.io_buf[..data.len()].copy_from_slice(data);
        });
    }

    fn read_staged(len: usize) -> Vec<u8> {
        with_rt(|rt| rt.io_buf[..len].to_vec())
    }

    #[test]
    fn abi_version_is_nonzero() {
        assert_eq!(wasmnux_abi_version(), 1);
    }

    #[test]
    fn wasm_abi_lifecycle_boot_uart_snapshot_and_error_bounds() {
        // 1. Out-of-bounds RAM must fail cleanly with structured error code
        stage_bytes(b"vanilla");
        assert_eq!(wasmnux_vm_create(4, 16, 1000, 0, 7), 0);
        assert_eq!(
            wasmnux_last_error_code(),
            WasmErrorCode::InvalidArgument as u32
        );
        let err_len = wasmnux_last_error_read();
        assert!(err_len > 0);

        // 2. Valid VM creation, SBI putchar + SRST kernel boot, snapshot/restore, and teardown
        stage_bytes(b"vanilla");
        let handle = wasmnux_vm_create(16, 4, 1_761_868_800, 0, 7);
        assert_ne!(handle, 0);

        let mut kernel = vec![0u8; 256];
        kernel[0..4].copy_from_slice(&0x0400_006Fu32.to_le_bytes());
        kernel[8..16].copy_from_slice(&KERNEL_LOAD_OFFSET.to_le_bytes());
        kernel[16..24].copy_from_slice(&4096u64.to_le_bytes());
        kernel[56..60].copy_from_slice(&RISCV_IMAGE_MAGIC2.to_le_bytes());
        let insns: &[u32] = &[
            0x0480_0513, // addi a0, zero, 'H'
            0x0010_0893, // addi a7, zero, 1
            0x0000_0073, // ecall
            0x0490_0513, // addi a0, zero, 'I'
            0x0010_0893, // addi a7, zero, 1
            0x0000_0073, // ecall
            0x5352_58B7, // lui a7, 0x53525
            0x3548_8893, // addi a7, a7, 0x354 ("SRST")
            0x0000_0813, // a6 = 0
            0x0000_0513, // a0 = 0 (PowerOff)
            0x0000_0073, // ecall
        ];
        for (i, &w) in insns.iter().enumerate() {
            let off = 64 + i * 4;
            kernel[off..off + 4].copy_from_slice(&w.to_le_bytes());
        }

        stage_bytes(&kernel);
        assert_eq!(wasmnux_vm_stage_kernel(handle, kernel.len()), 0);
        stage_bytes(b"console=ttyS0");
        assert_eq!(wasmnux_vm_boot(handle, 13), 0);

        // Execute 4 steps ('j +64' + putchar 'H')
        let packed1 = wasmnux_vm_step(handle, 4);
        assert_eq!(packed1 >> 28, 0);
        let out_len1 = wasmnux_uart_drain_output(handle);
        assert_eq!(read_staged(out_len1), b"H");

        // Save full snapshot (CPU+RAM + Disk overlay)
        let snap_len = wasmnux_vm_save_full_snapshot(handle);
        assert!(snap_len > 16 * 1024 * 1024);
        let saved_snap = read_staged(snap_len);

        // Run to SRST PowerOff
        let packed2 = wasmnux_vm_step(handle, 32);
        assert_eq!(packed2 >> 28, 2); // PowerOff!
        let out_len2 = wasmnux_uart_drain_output(handle);
        assert_eq!(read_staged(out_len2), b"I");

        // Restore full snapshot and confirm re-running emits 'I' again!
        stage_bytes(&saved_snap);
        assert_eq!(
            wasmnux_vm_restore_full_snapshot(handle, saved_snap.len()),
            0
        );
        let packed3 = wasmnux_vm_step(handle, 32);
        assert_eq!(packed3 >> 28, 2);
        let out_len3 = wasmnux_uart_drain_output(handle);
        assert_eq!(read_staged(out_len3), b"I");

        // Telemetry check
        assert_eq!(wasmnux_vm_telemetry(handle), 64);

        // Clean destroy
        assert_eq!(wasmnux_vm_destroy(handle), 0);
        assert_eq!(wasmnux_vm_destroy(handle), -1);
    }
}
