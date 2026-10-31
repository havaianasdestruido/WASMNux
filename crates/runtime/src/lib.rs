//! Versioned C ABI. All calls belong to one dedicated browser module worker.
//! Guest block I/O crosses the boundary synchronously into OPFS access handles.
use riscv_core::{BlockDevice, Entropy, Machine, MachineConfig};
use std::sync::{Arc, Mutex};
const ABI: u32 = 1;
#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "wnx")]
extern "C" {
    fn disk_read(offset: f64, ptr: *mut u8, len: u32) -> i32;
    fn disk_write(offset: f64, ptr: *const u8, len: u32) -> i32;
    fn disk_flush() -> i32;
    fn entropy(ptr: *mut u8, len: u32) -> i32;
}
#[cfg(not(target_arch = "wasm32"))]
unsafe fn disk_read(_: f64, _: *mut u8, _: u32) -> i32 {
    -1
}
#[cfg(not(target_arch = "wasm32"))]
unsafe fn disk_write(_: f64, _: *const u8, _: u32) -> i32 {
    -1
}
#[cfg(not(target_arch = "wasm32"))]
unsafe fn disk_flush() -> i32 {
    -1
}
#[cfg(not(target_arch = "wasm32"))]
unsafe fn entropy(_: *mut u8, _: u32) -> i32 {
    -1
}
struct HostDisk;
impl BlockDevice for HostDisk {
    fn read_at(&mut self, o: u64, b: &mut [u8]) -> Result<(), String> {
        if unsafe { disk_read(o as f64, b.as_mut_ptr(), b.len() as u32) } == 0 {
            Ok(())
        } else {
            Err("OPFS read failed".into())
        }
    }
    fn write_at(&mut self, o: u64, b: &[u8]) -> Result<(), String> {
        if unsafe { disk_write(o as f64, b.as_ptr(), b.len() as u32) } == 0 {
            Ok(())
        } else {
            Err("OPFS journaled write failed".into())
        }
    }
    fn flush(&mut self) -> Result<(), String> {
        if unsafe { disk_flush() } == 0 {
            Ok(())
        } else {
            Err("OPFS flush failed".into())
        }
    }
}
struct HostEntropy;
impl Entropy for HostEntropy {
    fn fill(&mut self, b: &mut [u8]) -> Result<(), String> {
        for chunk in b.chunks_mut(65536) {
            if unsafe { entropy(chunk.as_mut_ptr(), chunk.len() as u32) } != 0 {
                return Err("Web Crypto entropy failed".into());
            }
        }
        Ok(())
    }
}
pub struct Runtime {
    machine: Machine,
    cfg: MachineConfig,
    error: Option<String>,
}
fn checked_slice<'a>(ptr: *const u8, len: usize) -> Result<&'a [u8], String> {
    checked_slice_max(ptr, len, 64 * 1024 * 1024)
}
fn checked_slice_max<'a>(ptr: *const u8, len: usize, max: usize) -> Result<&'a [u8], String> {
    if ptr.is_null() || len > max {
        return Err("invalid asset pointer/length".into());
    }
    // WASM is a single linear address space; the host validates the same range before calling.
    Ok(unsafe { std::slice::from_raw_parts(ptr, len) })
}
#[no_mangle]
pub extern "C" fn wnx_abi_version() -> u32 {
    ABI
}
#[no_mangle]
pub extern "C" fn wnx_alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len);
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}
/// Safety: ptr/len must come from wnx_alloc and have no live aliases.
/// # Safety
/// `p` must be a live runtime handle (or null where documented); buffers must be
/// valid for the supplied length and originate from `wnx_alloc` with no live aliases.
#[no_mangle]
pub unsafe extern "C" fn wnx_free(ptr: *mut u8, len: usize) {
    if !ptr.is_null() {
        drop(Vec::from_raw_parts(ptr, len, len))
    }
}
/// # Safety
/// `p` must be a live runtime handle (or null where documented); buffers must be
/// valid for the supplied length and originate from `wnx_alloc` with no live aliases.
#[no_mangle]
pub unsafe extern "C" fn wnx_create(
    ram_mb: u32,
    sectors: f64,
    fw: *const u8,
    fw_len: usize,
    k: *const u8,
    k_len: usize,
    initrd: *const u8,
    initrd_len: usize,
    cmd: *const u8,
    cmd_len: usize,
) -> *mut Runtime {
    let make = || -> Result<Box<Runtime>, String> {
        if !sectors.is_finite()
            || !(128.0..=8_388_608.0).contains(&sectors)
            || sectors.fract() != 0.0
        {
            return Err("invalid disk capacity".into());
        }
        let cfg = MachineConfig {
            ram_bytes: ram_mb as u64 * 1024 * 1024,
            disk_sectors: sectors as u64,
            cmdline: String::from_utf8(checked_slice(cmd, cmd_len)?.to_vec())
                .map_err(|_| "invalid command line")?,
        };
        let m = Machine::new(
            &cfg,
            checked_slice(fw, fw_len)?,
            checked_slice(k, k_len)?,
            checked_slice(initrd, initrd_len)?,
            Arc::new(Mutex::new(HostDisk)),
            Arc::new(Mutex::new(HostEntropy)),
        )
        .map_err(|e| e.to_string())?;
        Ok(Box::new(Runtime {
            machine: m,
            cfg,
            error: None,
        }))
    };
    match make() {
        Ok(r) => Box::into_raw(r),
        Err(_) => std::ptr::null_mut(),
    }
}
fn runtime<'a>(p: *mut Runtime) -> Option<&'a mut Runtime> {
    if p.is_null() {
        None
    } else {
        Some(unsafe { &mut *p })
    }
}
/// # Safety
/// `p` must be a live runtime handle (or null where documented); buffers must be
/// valid for the supplied length and originate from `wnx_alloc` with no live aliases.
#[no_mangle]
pub unsafe extern "C" fn wnx_destroy(p: *mut Runtime) {
    if !p.is_null() {
        drop(Box::from_raw(p))
    }
}
/// # Safety
/// `p` must be a live runtime handle (or null where documented); buffers must be
/// valid for the supplied length and originate from `wnx_alloc` with no live aliases.
#[no_mangle]
pub unsafe extern "C" fn wnx_step(p: *mut Runtime, budget: u32) -> i64 {
    match runtime(p) {
        Some(r) => match r.machine.step(budget) {
            Ok(n) => n as i64,
            Err(e) => {
                r.error = Some(e.to_string());
                -1
            }
        },
        None => -2,
    }
}
/// # Safety
/// `p` must be a live runtime handle (or null where documented); buffers must be
/// valid for the supplied length and originate from `wnx_alloc` with no live aliases.
#[no_mangle]
pub unsafe extern "C" fn wnx_input(p: *mut Runtime, ptr: *const u8, len: usize) -> i32 {
    match runtime(p) {
        Some(r) => {
            if len > 65536 {
                -1
            } else {
                match checked_slice(ptr, len)
                    .map_err(|e| e.to_string())
                    .and_then(|b| r.machine.console.input(b).map_err(|e| e.to_string()))
                {
                    Ok(()) => 0,
                    Err(e) => {
                        r.error = Some(e);
                        -1
                    }
                }
            }
        }
        None => -2,
    }
}
/// # Safety
/// `p` must be a live runtime handle (or null where documented); buffers must be
/// valid for the supplied length and originate from `wnx_alloc` with no live aliases.
#[no_mangle]
pub unsafe extern "C" fn wnx_output(p: *mut Runtime, ptr: *mut u8, capacity: usize) -> i32 {
    match runtime(p) {
        Some(r) => {
            let out = r.machine.console.drain();
            if out.len() > capacity {
                return -(out.len() as i32);
            }
            std::ptr::copy_nonoverlapping(out.as_ptr(), ptr, out.len());
            out.len() as i32
        }
        None => -1,
    }
}
/// # Safety
/// `p` must be a live runtime handle (or null where documented); buffers must be
/// valid for the supplied length and originate from `wnx_alloc` with no live aliases.
#[no_mangle]
pub unsafe extern "C" fn wnx_powered_off(p: *mut Runtime) -> i32 {
    runtime(p).map_or(-1, |r| r.machine.powered_off as i32)
}
/// # Safety
/// `p` must be live; `ptr` must be a valid `wnx_alloc` buffer of at least `capacity` bytes.
#[no_mangle]
pub unsafe extern "C" fn wnx_framebuffer(p: *mut Runtime, ptr: *mut u8, capacity: usize) -> i32 {
    match runtime(p) {
        Some(r) => {
            if capacity < riscv_core::FB_SIZE || ptr.is_null() {
                return -1;
            }
            let off = r.machine.cpu.mem.offset(riscv_core::FB_BASE);
            r.machine.cpu.mem.read_bytes(
                off,
                std::slice::from_raw_parts_mut(ptr, riscv_core::FB_SIZE),
            );
            riscv_core::FB_SIZE as i32
        }
        None => -1,
    }
}
/// # Safety
/// `p` must be a live runtime handle or null.
#[no_mangle]
pub unsafe extern "C" fn wnx_instret(p: *mut Runtime) -> f64 {
    runtime(p).map_or(0.0, |r| r.machine.cpu.instret as f64)
}
/// # Safety
/// `p` must be a live runtime handle (or null where documented); buffers must be
/// valid for the supplied length and originate from `wnx_alloc` with no live aliases.
#[no_mangle]
pub unsafe extern "C" fn wnx_snapshot_size(p: *mut Runtime) -> i64 {
    match runtime(p) {
        Some(r) => match r.machine.save_state(&r.cfg) {
            Ok(b) => b.len() as i64,
            Err(e) => {
                r.error = Some(e);
                -1
            }
        },
        None => -2,
    }
}
/// # Safety
/// `p` must be a live runtime handle (or null where documented); buffers must be
/// valid for the supplied length and originate from `wnx_alloc` with no live aliases.
#[no_mangle]
pub unsafe extern "C" fn wnx_snapshot_save(p: *mut Runtime, ptr: *mut u8, capacity: usize) -> i64 {
    match runtime(p) {
        Some(r) => match r.machine.save_state(&r.cfg) {
            Ok(b) => {
                if b.len() > capacity {
                    return -(b.len() as i64);
                }
                std::ptr::copy_nonoverlapping(b.as_ptr(), ptr, b.len());
                b.len() as i64
            }
            Err(e) => {
                r.error = Some(e);
                -1
            }
        },
        None => -2,
    }
}
/// # Safety
/// `p` must be a live runtime handle (or null where documented); buffers must be
/// valid for the supplied length and originate from `wnx_alloc` with no live aliases.
#[no_mangle]
pub unsafe extern "C" fn wnx_snapshot_load(p: *mut Runtime, ptr: *const u8, len: usize) -> i32 {
    match runtime(p) {
        Some(r) => match checked_slice(ptr, len)
            .map_err(|e| e.to_string())
            .and_then(|b| r.machine.load_state(&r.cfg, b))
        {
            Ok(()) => 0,
            Err(e) => {
                r.error = Some(e);
                -1
            }
        },
        None => -2,
    }
}
/// # Safety
/// `p` must be a live runtime handle (or null where documented); buffers must be
/// valid for the supplied length and originate from `wnx_alloc` with no live aliases.
#[no_mangle]
pub unsafe extern "C" fn wnx_error(p: *mut Runtime, ptr: *mut u8, capacity: usize) -> i32 {
    match runtime(p).and_then(|r| r.error.as_ref()) {
        Some(e) => {
            let b = e.as_bytes();
            if b.len() > capacity {
                return -(b.len() as i32);
            }
            std::ptr::copy_nonoverlapping(b.as_ptr(), ptr, b.len());
            b.len() as i32
        }
        None => 0,
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn abi_version() {
        assert_eq!(super::wnx_abi_version(), 1);
    }
    #[test]
    fn null_handles_rejected() {
        unsafe {
            assert_eq!(super::wnx_step(std::ptr::null_mut(), 1), -2);
            assert_eq!(super::wnx_powered_off(std::ptr::null_mut()), -1);
            super::wnx_destroy(std::ptr::null_mut());
        }
    }
}
