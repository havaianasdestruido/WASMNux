//! Minimal 16550-compatible UART (modelled as a byte sink).
//!
//! Memory-mapped at 0x1000_0000 (qemu "virt" uart0). Writing a byte to the
//! transmit holding register delivers it to the output sink (the guest
//! serial console). Reads return status/line-status bits.

/// UART base address (qemu "virt" map).
pub const UART_BASE: u64 = 0x1000_0000;
/// UART register block size.
pub const UART_SIZE: u64 = 0x1000;

// Register offsets (within the 0x1000 block)
pub const UART_THR: u64 = 0x00; // Transmit Holding Register (write)
const UART_RBR: u64 = 0x00; // Receiver Buffer Register (read)
const UART_LSR: u64 = 0x05; // Line Status Register (read)
const LSR_THRE: u8 = 1 << 5; // Transmit Holding Register Empty

/// A UART device with a byte output sink and an optional host putchar callback.
pub struct Uart {
    pub output: Vec<u8>,
    /// Optional callback invoked on each transmitted byte (used by WASM host).
    pub putchar: Option<extern "C" fn(u8)>,
}

impl Uart {
    pub fn new() -> Self {
        Self {
            output: Vec::new(),
            putchar: None,
        }
    }

    pub fn set_putchar(&mut self, cb: extern "C" fn(u8)) {
        self.putchar = Some(cb);
    }

    /// Address decode: returns the register offset if `addr` is in the UART block.
    fn decode(addr: u64) -> Option<u64> {
        if addr < UART_BASE {
            return None;
        }
        let off = addr - UART_BASE;
        if off < UART_SIZE {
            return Some(off);
        }
        None
    }

    /// Read a byte from `addr`. Returns `None` if not a UART address.
    pub fn read_byte(&self, addr: u64) -> Option<u8> {
        let off = Self::decode(addr)?;
        match off {
            UART_RBR => Some(0),        // no data available
            UART_LSR => Some(LSR_THRE), // transmitter always ready
            _ => Some(0),
        }
    }

    /// Write a byte to `addr`. Returns `true` if handled by the UART.
    pub fn write_byte(&mut self, addr: u64, value: u8) -> bool {
        let off = match Self::decode(addr) {
            Some(o) => o,
            None => return false,
        };
        if off == UART_THR {
            self.output.push(value);
            if let Some(cb) = self.putchar {
                cb(value);
            }
            return true;
        }
        true // other UART registers are write-ignored
    }
}

impl Default for Uart {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thr_write_delivers_byte() {
        let mut uart = Uart::new();
        assert!(uart.write_byte(UART_BASE + UART_THR, b'A'));
        assert_eq!(uart.output, vec![b'A']);
    }

    #[test]
    fn lsr_read_ready() {
        let uart = Uart::new();
        assert_eq!(uart.read_byte(UART_BASE + UART_LSR).unwrap(), LSR_THRE);
    }

    #[test]
    fn outside_uart_rejected() {
        let mut uart = Uart::new();
        assert!(uart.read_byte(0).is_none());
        assert!(!uart.write_byte(0, 0));
    }

    #[test]
    fn putchar_callback_invoked() {
        static mut CALLED: bool = false;
        static mut CALLED_BYTE: u8 = 0;
        extern "C" fn cb(b: u8) {
            unsafe {
                CALLED = true;
                CALLED_BYTE = b;
            }
        }
        let mut uart = Uart::new();
        uart.set_putchar(cb);
        uart.write_byte(UART_BASE + UART_THR, b'Z');
        assert!(unsafe { CALLED });
        assert_eq!(unsafe { CALLED_BYTE }, b'Z');
    }
}
