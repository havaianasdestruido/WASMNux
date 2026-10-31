#![allow(dead_code)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

//! Raw WebAssembly C ABI exports.
//!
//! These are called directly from TypeScript via the WebAssembly JavaScript
//! API (no `wasm-bindgen`). Guest memory lives in the WASM linear memory, so
//! TypeScript can read/write it through the exported `memory` buffer using
//! the pointers returned here.

use crate::machine::{Machine, MachineConfig, MachineError};

#[no_mangle]
pub extern "C" fn wasmnux_machine_new(mem_size: usize, harts: usize) -> *mut Machine {
    let config = MachineConfig {
        mem_size,
        harts,
        ..MachineConfig::default()
    };
    Box::into_raw(Box::new(Machine::new(config)))
}

#[no_mangle]
pub extern "C" fn wasmnux_machine_free(ptr: *mut Machine) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(ptr));
    }
}

#[no_mangle]
pub extern "C" fn wasmnux_machine_reset(ptr: *mut Machine) {
    let m = match unsafe { ptr.as_mut() } {
        Some(m) => m,
        None => return,
    };
    m.reset();
}

#[no_mangle]
pub extern "C" fn wasmnux_machine_halt(ptr: *mut Machine) {
    if let Some(m) = unsafe { ptr.as_mut() } {
        m.halt();
    }
}

#[no_mangle]
pub extern "C" fn wasmnux_machine_load_elf(
    ptr: *mut Machine,
    image_ptr: *const u8,
    image_len: usize,
    entry_out: *mut u64,
) -> i32 {
    let m = match unsafe { ptr.as_mut() } {
        Some(m) => m,
        None => return -1,
    };
    let image = unsafe { std::slice::from_raw_parts(image_ptr, image_len) };
    match m.load_elf(image) {
        Ok(entry) => {
            if !entry_out.is_null() {
                unsafe { *entry_out = entry };
            }
            0
        }
        Err(e) => {
            // -2 = ELF error
            match e {
                MachineError::Elf(_) => -2,
                _ => -3,
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn wasmnux_machine_load_device_tree(ptr: *mut Machine, addr_out: *mut u64) -> i32 {
    let m = match unsafe { ptr.as_mut() } {
        Some(m) => m,
        None => return -1,
    };
    match m.load_device_tree() {
        Ok(addr) => {
            if !addr_out.is_null() {
                unsafe { *addr_out = addr };
            }
            0
        }
        Err(_) => -4,
    }
}

#[no_mangle]
pub extern "C" fn wasmnux_machine_set_boot_args(ptr: *mut Machine, hart_id: u64, dtb_addr: u64) {
    if let Some(m) = unsafe { ptr.as_mut() } {
        m.set_boot_args(hart_id, dtb_addr);
    }
}

#[no_mangle]
pub extern "C" fn wasmnux_machine_step(ptr: *mut Machine) -> i32 {
    let m = match unsafe { ptr.as_mut() } {
        Some(m) => m,
        None => return -1,
    };
    match m.step() {
        Ok(_) => 0,
        Err(MachineError::Trap(_)) => 1, // guest trap
        Err(_) => 2,                     // other error
    }
}

#[no_mangle]
pub extern "C" fn wasmnux_machine_run(ptr: *mut Machine, max_instructions: u64) -> i32 {
    let m = match unsafe { ptr.as_mut() } {
        Some(m) => m,
        None => return -1,
    };
    match m.run(max_instructions) {
        Ok(_) => 0,
        Err(_) => 2,
    }
}

#[no_mangle]
pub extern "C" fn wasmnux_machine_mem_ptr(ptr: *mut Machine) -> *mut u8 {
    let m = match unsafe { ptr.as_mut() } {
        Some(m) => m,
        None => return std::ptr::null_mut(),
    };
    m.bus.mem.as_mut_slice().as_mut_ptr()
}

#[no_mangle]
pub extern "C" fn wasmnux_machine_mem_size(ptr: *mut Machine) -> u64 {
    match unsafe { ptr.as_ref() } {
        Some(m) => m.bus.mem.len(),
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn wasmnux_machine_pc(ptr: *mut Machine) -> u64 {
    match unsafe { ptr.as_ref() } {
        Some(m) => m.pc(),
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn wasmnux_machine_reg(ptr: *mut Machine, idx: u32) -> u64 {
    unsafe { ptr.as_ref() }
        .and_then(|m| m.reg(idx as usize))
        .unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn wasmnux_machine_cycles(ptr: *mut Machine) -> u64 {
    match unsafe { ptr.as_ref() } {
        Some(m) => m.cycles(),
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn wasmnux_machine_mtime(ptr: *mut Machine) -> u64 {
    match unsafe { ptr.as_ref() } {
        Some(m) => m.mtime(),
        None => 0,
    }
}

/// Drain console bytes into a host buffer. Returns number of bytes written.
#[no_mangle]
pub extern "C" fn wasmnux_machine_drain_console(
    ptr: *mut Machine,
    out_ptr: *mut u8,
    out_len: usize,
) -> i32 {
    let m = match unsafe { ptr.as_mut() } {
        Some(m) => m,
        None => return -1,
    };
    let bytes = m.drain_console();
    let n = bytes.len().min(out_len);
    if n > 0 {
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), out_ptr, n);
        }
    }
    n as i32
}

/// Write a host buffer into guest memory. Returns 0 on success.
#[no_mangle]
pub extern "C" fn wasmnux_machine_write_mem(
    ptr: *mut Machine,
    addr: u64,
    src: *const u8,
    len: usize,
) -> i32 {
    let m = match unsafe { ptr.as_mut() } {
        Some(m) => m,
        None => return -1,
    };
    let src = unsafe { std::slice::from_raw_parts(src, len) };
    m.write_mem(addr, src).map(|()| 0).unwrap_or(-5)
}
