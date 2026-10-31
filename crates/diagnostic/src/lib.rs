//! Bounded, single-instance ABI for the historical diagnostic emulator.
//!
//! The host copies already-verified guest images into a Rust-owned input
//! buffer. No host pointer is accepted by an exported function. This is not a
//! persistent VM or a stable, general-purpose virtualization API.
#![deny(unsafe_op_in_unsafe_fn)]
// Export names are deliberately unique to this ABI; no Rust pointer is accepted.

use riscv_emu_rust::{terminal::Terminal, Emulator};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
};

const OK: i32 = 0;
const BAD_STATE: i32 = 1;
const BAD_ARGUMENT: i32 = 2;
const QUEUE_FULL: i32 = 3;
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_INPUT_BYTES: usize = 16 * 1024;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_CYCLES: u32 = 100_000;

#[derive(Default)]
struct SerialQueues {
    input: VecDeque<u8>,
    output: VecDeque<u8>,
    input_overflowed: bool,
    output_overflowed: bool,
}

struct QueuedTerminal(Rc<RefCell<SerialQueues>>);

impl Terminal for QueuedTerminal {
    fn put_byte(&mut self, value: u8) {
        let mut queues = self.0.borrow_mut();
        if queues.output.len() >= MAX_OUTPUT_BYTES {
            queues.output_overflowed = true;
        } else {
            queues.output.push_back(value);
        }
    }

    fn get_output(&mut self) -> u8 {
        self.0.borrow_mut().output.pop_front().unwrap_or(0)
    }

    fn put_input(&mut self, value: u8) {
        let mut queues = self.0.borrow_mut();
        if queues.input.len() < MAX_INPUT_BYTES {
            queues.input.push_back(value);
        } else {
            queues.input_overflowed = true;
        }
    }

    fn get_input(&mut self) -> u8 {
        self.0.borrow_mut().input.pop_front().unwrap_or(0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Stage {
    New,
    ProgramLoaded,
    Ready,
}

struct Session {
    emulator: Emulator,
    queues: Rc<RefCell<SerialQueues>>,
    stage: Stage,
    input_buffer: Option<Vec<u8>>,
}

impl Session {
    fn new() -> Self {
        let queues = Rc::new(RefCell::new(SerialQueues::default()));
        let emulator = Emulator::new(Box::new(QueuedTerminal(queues.clone())));
        Self {
            emulator,
            queues,
            stage: Stage::New,
            input_buffer: None,
        }
    }
}

thread_local! {
    // Each browser worker owns a separate Wasm instance and session.
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
    // Kept separate from SESSION so it remains readable after a Wasm trap.
    static LAST_PC: Cell<u64> = const { Cell::new(0) };
}

fn with_session<T>(otherwise: T, f: impl FnOnce(&mut Session) -> T) -> T {
    SESSION.with(|slot| match slot.try_borrow_mut() {
        Ok(mut slot) => match slot.as_mut() {
            Some(session) => f(session),
            None => otherwise,
        },
        Err(_) => otherwise,
    })
}

/// Create exactly one emulator in this Wasm instance.
#[unsafe(no_mangle)]
pub extern "C" fn vm_new() -> i32 {
    SESSION.with(|slot| match slot.try_borrow_mut() {
        Ok(mut slot) if slot.is_none() => {
            LAST_PC.with(|last_pc| last_pc.set(0));
            *slot = Some(Session::new());
            OK
        }
        _ => BAD_STATE,
    })
}

/// Reserve a Rust-owned image buffer and return its Wasm-memory offset, or 0.
#[unsafe(no_mangle)]
pub extern "C" fn vm_alloc_image(length: u32) -> usize {
    with_session(0, |session| {
        let length = length as usize;
        if session.stage == Stage::Ready
            || session.input_buffer.is_some()
            || length == 0
            || length > MAX_IMAGE_BYTES
        {
            return 0;
        }
        let mut bytes = Vec::new();
        if bytes.try_reserve_exact(length).is_err() {
            return 0;
        }
        bytes.resize(length, 0);
        let offset = bytes.as_mut_ptr() as usize;
        session.input_buffer = Some(bytes);
        offset
    })
}

/// Consume the reserved image as the firmware/kernel payload.
#[unsafe(no_mangle)]
pub extern "C" fn vm_load_program() -> i32 {
    with_session(BAD_STATE, |session| {
        if session.stage != Stage::New {
            return BAD_STATE;
        }
        match session.input_buffer.take() {
            Some(bytes) => {
                session.emulator.setup_program(bytes);
                session.stage = Stage::ProgramLoaded;
                OK
            }
            None => BAD_STATE,
        }
    })
}

/// Consume the reserved image as the volatile guest block device.
#[unsafe(no_mangle)]
pub extern "C" fn vm_load_disk() -> i32 {
    with_session(BAD_STATE, |session| {
        if session.stage != Stage::ProgramLoaded {
            return BAD_STATE;
        }
        match session.input_buffer.take() {
            Some(bytes) => {
                if bytes.len() % 512 != 0 {
                    return BAD_ARGUMENT;
                }
                session.emulator.setup_filesystem(bytes);
                // Upstream describes this cache as experimental and unsound
                // when guest page tables change.
                session.emulator.enable_page_cache(false);
                session.stage = Stage::Ready;
                OK
            }
            None => BAD_STATE,
        }
    })
}

/// Run one bounded scheduling slice. Overflow is an error, never silent loss.
#[unsafe(no_mangle)]
pub extern "C" fn vm_run_cycles(cycles: u32) -> i32 {
    with_session(BAD_STATE, |session| {
        if session.stage != Stage::Ready || session.input_buffer.is_some() {
            return BAD_STATE;
        }
        if cycles == 0 || cycles > MAX_CYCLES {
            return BAD_ARGUMENT;
        }
        LAST_PC.with(|last_pc| {
            for _ in 0..cycles {
                last_pc.set(session.emulator.get_cpu().read_pc());
                session.emulator.tick();
            }
        });
        let queues = session.queues.borrow();
        if queues.input_overflowed || queues.output_overflowed {
            QUEUE_FULL
        } else {
            OK
        }
    })
}

/// Send one serial input byte, returning an error if the queue is full.
#[unsafe(no_mangle)]
pub extern "C" fn vm_put_input(byte: u32) -> i32 {
    with_session(BAD_STATE, |session| {
        if session.stage != Stage::Ready {
            return BAD_STATE;
        }
        if byte > u8::MAX as u32 {
            return BAD_ARGUMENT;
        }
        let mut queues = session.queues.borrow_mut();
        if queues.input.len() >= MAX_INPUT_BYTES {
            return QUEUE_FULL;
        }
        queues.input.push_back(byte as u8);
        OK
    })
}

/// Read an output byte, -1 when empty, or -2 if no session exists.
#[unsafe(no_mangle)]
pub extern "C" fn vm_get_output() -> i32 {
    with_session(-2, |session| {
        session
            .queues
            .borrow_mut()
            .output
            .pop_front()
            .map_or(-1, i32::from)
    })
}

/// Last attempted guest program counter, readable even after a Wasm trap.
#[unsafe(no_mangle)]
pub extern "C" fn vm_last_pc() -> u64 {
    LAST_PC.with(Cell::get)
}

/// Drop the session; the enclosing worker remains responsible for termination.
#[unsafe(no_mangle)]
pub extern "C" fn vm_free() -> i32 {
    SESSION.with(|slot| match slot.try_borrow_mut() {
        Ok(mut slot) => {
            *slot = None;
            OK
        }
        Err(_) => BAD_STATE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abi_rejects_invalid_lifecycle_and_image_sizes() {
        assert_eq!(vm_free(), OK);
        assert_eq!(vm_load_program(), BAD_STATE);
        assert_eq!(vm_new(), OK);
        assert_eq!(vm_new(), BAD_STATE);
        assert_eq!(vm_alloc_image(0), 0);
        assert_eq!(vm_alloc_image((MAX_IMAGE_BYTES + 1) as u32), 0);
        assert_ne!(vm_alloc_image(4), 0);
        assert_eq!(vm_alloc_image(4), 0);
        assert_eq!(vm_load_disk(), BAD_STATE);
        assert_eq!(vm_run_cycles(1), BAD_STATE);
        assert_eq!(vm_put_input(0), BAD_STATE);
        assert_eq!(vm_free(), OK);
        assert_eq!(vm_get_output(), -2);
        assert_eq!(vm_free(), OK);
    }

    #[test]
    fn abi_rejects_a_partial_disk_sector() {
        assert_eq!(vm_free(), OK);
        assert_eq!(vm_new(), OK);
        SESSION.with(|slot| slot.borrow_mut().as_mut().unwrap().stage = Stage::ProgramLoaded);
        assert_ne!(vm_alloc_image(513), 0);
        assert_eq!(vm_load_disk(), BAD_ARGUMENT);
        assert_eq!(vm_run_cycles(1), BAD_STATE);
        // The rejected buffer is released and a replacement can be reserved.
        assert_ne!(vm_alloc_image(512), 0);
        assert_eq!(vm_free(), OK);
    }

    #[test]
    fn abi_rejects_bad_cycles_and_full_serial_input() {
        assert_eq!(vm_free(), OK);
        assert_eq!(vm_new(), OK);
        // Exercise boundary validation without loading an unverified image.
        SESSION.with(|slot| slot.borrow_mut().as_mut().unwrap().stage = Stage::Ready);
        assert_eq!(vm_run_cycles(0), BAD_ARGUMENT);
        assert_eq!(vm_run_cycles(MAX_CYCLES + 1), BAD_ARGUMENT);
        assert_eq!(vm_put_input(256), BAD_ARGUMENT);
        for _ in 0..MAX_INPUT_BYTES {
            assert_eq!(vm_put_input(7), OK);
        }
        assert_eq!(vm_put_input(7), QUEUE_FULL);
        assert_eq!(vm_last_pc(), 0);
        assert_eq!(vm_free(), OK);
    }

    #[test]
    fn terminal_queues_are_bounded_and_preserve_zero_bytes() {
        let queues = Rc::new(RefCell::new(SerialQueues::default()));
        let mut terminal = QueuedTerminal(queues.clone());
        for _ in 0..MAX_INPUT_BYTES {
            terminal.put_input(1);
        }
        terminal.put_input(2);
        assert_eq!(queues.borrow().input.len(), MAX_INPUT_BYTES);
        assert!(queues.borrow().input_overflowed);
        assert_eq!(terminal.get_input(), 1);

        terminal.put_byte(0);
        terminal.put_byte(7);
        assert_eq!(queues.borrow_mut().output.pop_front(), Some(0));
        assert_eq!(queues.borrow_mut().output.pop_front(), Some(7));
        for _ in 0..MAX_OUTPUT_BYTES {
            terminal.put_byte(3);
        }
        terminal.put_byte(4);
        assert_eq!(queues.borrow().output.len(), MAX_OUTPUT_BYTES);
        assert!(queues.borrow().output_overflowed);
    }
}
