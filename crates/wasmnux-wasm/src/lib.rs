//! WebAssembly surface for the WASMNux virtual machine. Everything here is
//! a thin, fallible bridge to `wasmnux_core`: JS receives real errors, never
//! silent fallbacks into fake behaviour.

use wasm_bindgen::prelude::{wasm_bindgen, JsValue};
use wasmnux_core::{
    BootMode, Machine as CoreMachine, MachineConfig, RunReason, VmError,
};

fn to_js_error(error: VmError) -> JsValue {
    JsValue::from_str(&error.to_string())
}

/// Core version, for UI diagnostics.
#[wasm_bindgen]
#[must_use]
pub fn core_version() -> String {
    wasmnux_core::VERSION.to_owned()
}

/// CRC-32 (same polynomial as the TS chunk store) for boundary verification.
#[wasm_bindgen]
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    wasmnux_core::crc::crc32(data)
}

/// A virtual machine instance living inside a Web Worker.
#[wasm_bindgen]
pub struct Machine {
    inner: CoreMachine,
}

#[wasm_bindgen]
impl Machine {
    /// Creates a machine. `boot_mode` is `"firmware"` (OpenSBI ELF first)
    /// or `"direct-sbi"` (kernel + built-in SBI shim).
    #[wasm_bindgen(constructor)]
    pub fn new(
        ram_mib: u32,
        bootargs: String,
        boot_mode: String,
        framebuffer: bool,
    ) -> Result<Machine, JsValue> {
        let mode = match boot_mode.as_str() {
            "firmware" => BootMode::Firmware,
            "direct-sbi" => BootMode::DirectSbi,
            other => return Err(JsValue::from_str(&format!("unknown boot mode: {other}"))),
        };
        let config = MachineConfig {
            ram_size_bytes: (ram_mib as usize) * 1024 * 1024,
            bootargs,
            framebuffer: if framebuffer { Some((1024, 600)) } else { None },
            boot_mode: mode,
        };
        Ok(Self {
            inner: CoreMachine::new(config).map_err(to_js_error)?,
        })
    }

    /// Lifecycle state: created | running | paused | halted.
    pub fn state(&self) -> String {
        self.inner.state().as_str().to_owned()
    }

    /// Loads a raw RISC-V kernel `Image`.
    pub fn load_kernel(&mut self, image: Vec<u8>) -> Result<(), JsValue> {
        self.inner.load_kernel_image(image).map_err(to_js_error)
    }

    /// Loads an OpenSBI-compatible firmware ELF (`fw_jump.elf`).
    pub fn load_firmware(&mut self, elf: Vec<u8>) -> Result<(), JsValue> {
        self.inner.load_firmware_elf(elf).map_err(to_js_error)
    }

    /// Loads an initramfs (gzip-compressed newc cpio, as built by guest/).
    pub fn load_initrd(&mut self, initrd: Vec<u8>) -> Result<(), JsValue> {
        self.inner.load_initrd(initrd).map_err(to_js_error)
    }

    /// Attaches a disk image (sector-aligned bytes from the chunk store).
    pub fn attach_disk(&mut self, image: Vec<u8>) -> Result<(), JsValue> {
        self.inner.attach_disk_image(image).map_err(to_js_error)
    }

    /// Creates a fresh zeroed disk of `size_mib` MiB.
    pub fn create_disk(&mut self, size_mib: u32) -> Result<(), JsValue> {
        self.inner
            .create_disk((size_mib as usize) * 1024 * 1024)
            .map_err(to_js_error)
    }

    /// Boots the machine.
    pub fn boot(&mut self) -> Result<(), JsValue> {
        self.inner.boot().map_err(to_js_error)
    }

    pub fn pause(&mut self) -> Result<(), JsValue> {
        self.inner.pause().map_err(to_js_error)
    }

    pub fn resume(&mut self) -> Result<(), JsValue> {
        self.inner.resume().map_err(to_js_error)
    }

    pub fn power_off(&mut self) -> Result<(), JsValue> {
        self.inner.power_off().map_err(to_js_error)
    }

    /// Runs up to `max_instructions` guest instructions. Returns the reason
    /// the slice ended: `slice` | `waiting` | `shutdown` | `reboot`.
    pub fn run_slice(&mut self, max_instructions: u32) -> Result<String, JsValue> {
        let report = self
            .inner
            .run_slice(u64::from(max_instructions))
            .map_err(to_js_error)?;
        let reason = match report.reason {
            RunReason::SliceExhausted => "slice",
            RunReason::WaitingForInterrupt => "waiting",
            RunReason::Shutdown => "shutdown",
            RunReason::Reboot => "reboot",
        };
        Ok(reason.to_owned())
    }

    /// Total guest instructions retired since creation.
    pub fn instructions(&self) -> u64 {
        self.inner.total_instructions()
    }

    /// Guest program counter (diagnostics / hang investigation).
    pub fn pc(&self) -> u64 {
        self.inner.cpu().pc()
    }

    /// Sends terminal input to the guest UART. `false` = retry later.
    pub fn serial_write(&mut self, bytes: &[u8]) -> bool {
        self.inner.write_serial_input(bytes)
    }

    /// Drains guest serial output for the terminal.
    pub fn serial_read(&mut self) -> Vec<u8> {
        self.inner.read_serial_output()
    }

    /// Whether the guest modified the disk since it was last persisted.
    pub fn disk_dirty(&self) -> bool {
        self.inner.disk_dirty()
    }

    /// The current disk image for the persistent chunk store.
    pub fn export_disk(&self) -> Option<Vec<u8>> {
        self.inner.disk_image().map(<[u8]>::to_vec)
    }

    /// Marks the disk clean after the host persisted `export_disk`.
    pub fn mark_disk_clean(&mut self) {
        self.inner.mark_disk_clean();
    }

    /// Framebuffer geometry as `[width, height, stride_bytes]`, if enabled.
    pub fn framebuffer_info(&self) -> Option<Vec<u32>> {
        self.inner
            .framebuffer()
            .map(|fb| vec![fb.width, fb.height, fb.stride])
    }

    /// A copy of the current framebuffer (x8r8g8b8) for presentation.
    pub fn framebuffer_bytes(&self) -> Option<Vec<u8>> {
        self.inner.framebuffer_bytes().map(<[u8]>::to_vec)
    }
}
