//! NS16550A UART (the guest serial console). Register semantics follow the
//! 16550A datasheet subset that Linux's `8250` driver exercises: DLAB bank
//! switching, interrupt identification/enable, FIFO control (modelled as
//! always-enabled queues), line/modern status. Output bytes go to a queue
//! the host drains into the browser terminal; input bytes from the terminal
//! are queued for the guest, and asserting an interrupt when data arrives.

use std::collections::VecDeque;

use crate::bus::MmioDevice;
use crate::error::{Result, VmError};

const REG_RBR_THR: u64 = 0;
const REG_IER: u64 = 1;
const REG_IIR_FCR: u64 = 2;
const REG_LCR: u64 = 3;
const REG_MCR: u64 = 4;
const REG_LSR: u64 = 5;
const REG_MSR: u64 = 6;
const REG_SCR: u64 = 7;

const LSR_DATA_READY: u8 = 1 << 0;
const LSR_THR_EMPTY: u8 = 1 << 5;
const LSR_TRANSMITTER_EMPTY: u8 = 1 << 6;

const IER_RX_AVAILABLE: u8 = 1 << 0;
const IER_THR_EMPTY: u8 = 1 << 1;

const IIR_NO_INTERRUPT: u8 = 0x01;
const IIR_THR_EMPTY: u8 = 0x02;
const IIR_RX_AVAILABLE: u8 = 0x04;
const IIR_FIFO_ENABLED: u8 = 0xC0;

/// Maximum bytes buffered from the host terminal.
const INPUT_CAPACITY: usize = 64 * 1024;

/// NS16550A-compatible UART.
#[derive(Debug)]
pub struct Uart16550 {
    ier: u8,
    lcr: u8,
    mcr: u8,
    scr: u8,
    divisor_lo: u8,
    divisor_hi: u8,
    input: VecDeque<u8>,
    output: VecDeque<u8>,
    thr_irq_pending: bool,
}

impl Default for Uart16550 {
    fn default() -> Self {
        Self::new()
    }
}

impl Uart16550 {
    pub fn new() -> Self {
        Self {
            ier: 0,
            lcr: 0,
            mcr: 0,
            scr: 0,
            divisor_lo: 1,
            divisor_hi: 0,
            input: VecDeque::new(),
            output: VecDeque::new(),
            thr_irq_pending: false,
        }
    }

    fn dlab(&self) -> bool {
        self.lcr & 0x80 != 0
    }

    /// Queues host (terminal) input for the guest. Returns `false` if the
    /// bounded queue is full; bytes are never silently partially accepted.
    pub fn host_write(&mut self, bytes: &[u8]) -> bool {
        if self.input.len() + bytes.len() > INPUT_CAPACITY {
            return false;
        }
        self.input.extend(bytes.iter().copied());
        true
    }

    /// Writes a single host-originated byte (SBI console path).
    pub fn host_push_output(&mut self, byte: u8) {
        self.output.push_back(byte);
    }

    /// Drains up to `buf.len()` guest output bytes for the host terminal.
    pub fn host_read(&mut self, buf: &mut [u8]) -> usize {
        let mut count = 0;
        while count < buf.len() {
            match self.output.pop_front() {
                Some(byte) => {
                    buf[count] = byte;
                    count += 1;
                }
                None => break,
            }
        }
        count
    }

    /// Drains all pending guest output.
    pub fn take_output(&mut self) -> Vec<u8> {
        self.output.drain(..).collect()
    }

    /// Whether the guest input queue is empty (used by SBI getchar).
    pub fn host_pop_input(&mut self) -> Option<u8> {
        self.input.pop_front()
    }

    pub fn input_len(&self) -> usize {
        self.input.len()
    }

    pub fn output_len(&self) -> usize {
        self.output.len()
    }

    fn iir(&mut self) -> u8 {
        if self.ier & IER_RX_AVAILABLE != 0 && !self.input.is_empty() {
            IIR_FIFO_ENABLED | IIR_RX_AVAILABLE
        } else if self.ier & IER_THR_EMPTY != 0 && self.thr_irq_pending {
            self.thr_irq_pending = false;
            IIR_FIFO_ENABLED | IIR_THR_EMPTY
        } else {
            IIR_FIFO_ENABLED | IIR_NO_INTERRUPT
        }
    }

    /// Whether the UART is asserting its interrupt line.
    pub fn irq_pending(&self) -> bool {
        (self.ier & IER_RX_AVAILABLE != 0 && !self.input.is_empty())
            || (self.ier & IER_THR_EMPTY != 0 && self.thr_irq_pending)
    }

    fn read_register(&mut self, offset: u64) -> u8 {
        match offset {
            REG_RBR_THR => {
                if self.dlab() {
                    self.divisor_lo
                } else {
                    self.input.pop_front().unwrap_or(0)
                }
            }
            REG_IER => {
                if self.dlab() {
                    self.divisor_hi
                } else {
                    self.ier
                }
            }
            REG_IIR_FCR => self.iir(),
            REG_LCR => self.lcr,
            REG_MCR => self.mcr,
            REG_LSR => {
                let mut status = LSR_THR_EMPTY | LSR_TRANSMITTER_EMPTY;
                if !self.input.is_empty() {
                    status |= LSR_DATA_READY;
                }
                status
            }
            REG_MSR => 0,
            REG_SCR => self.scr,
            _ => 0,
        }
    }

    fn write_register(&mut self, offset: u64, value: u8) {
        match offset {
            REG_RBR_THR => {
                if self.dlab() {
                    self.divisor_lo = value;
                } else {
                    self.output.push_back(value);
                    self.thr_irq_pending = false;
                }
            }
            REG_IER => {
                if self.dlab() {
                    self.divisor_hi = value;
                } else {
                    let enabling_thr_irq =
                        self.ier & IER_THR_EMPTY == 0 && value & IER_THR_EMPTY != 0;
                    self.ier = value & 0x0F;
                    if enabling_thr_irq {
                        self.thr_irq_pending = true;
                    }
                }
            }
            REG_IIR_FCR => {
                // FIFO control: bit 1 clears the receive FIFO, bit 2 the
                // transmit queue. Trigger levels are informational here.
                if value & 0x02 != 0 {
                    self.input.clear();
                }
                if value & 0x04 != 0 {
                    self.output.clear();
                }
            }
            REG_LCR => self.lcr = value,
            REG_MCR => self.mcr = value,
            REG_SCR => self.scr = value,
            _ => {}
        }
    }
}

impl MmioDevice for Uart16550 {
    fn read(&mut self, offset: u64, size: usize) -> Result<u64> {
        if size != 1 && size != 4 {
            return Err(VmError::UnsupportedAccessSize { size });
        }
        Ok(u64::from(self.read_register(offset)))
    }

    fn write(&mut self, offset: u64, size: usize, value: u64) -> Result<()> {
        if size != 1 && size != 4 {
            return Err(VmError::UnsupportedAccessSize { size });
        }
        self.write_register(offset, (value & 0xFF) as u8);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Uart16550;
    use crate::bus::MmioDevice;

    #[test]
    fn guest_output_reaches_host_and_host_input_reaches_guest() {
        let mut uart = Uart16550::new();
        uart.write(0, 1, u64::from(b'H')).expect("thr H");
        uart.write(0, 1, u64::from(b'i')).expect("thr i");
        assert_eq!(uart.take_output(), b"Hi".to_vec());

        assert!(uart.host_write(b"x"));
        assert_eq!(
            uart.read(5, 1).expect("lsr") & 1,
            1,
            "data-ready must be set"
        );
        assert_eq!(uart.read(0, 1).expect("rbr"), u64::from(b'x'));
        assert_eq!(uart.read(5, 1).expect("lsr") & 1, 0);
    }

    #[test]
    fn divisor_latch_banking_via_dlab() {
        let mut uart = Uart16550::new();
        uart.write(3, 1, 0x80).expect("set DLAB");
        uart.write(0, 1, 0x0C).expect("dll");
        uart.write(1, 1, 0x00).expect("dlm");
        assert_eq!(uart.read(0, 1).expect("dll read"), 0x0C);
        uart.write(3, 1, 0x03).expect("clear DLAB, 8n1");
        assert_eq!(
            uart.output_len(),
            0,
            "DLAB writes must not emit UART output"
        );
    }

    #[test]
    fn receive_interrupt_follows_ier_and_input() {
        let mut uart = Uart16550::new();
        assert!(uart.host_write(b"a"));
        assert!(!uart.irq_pending(), "IER=0 must not assert IRQ");
        uart.write(1, 1, 0x01).expect("enable RX irq");
        assert!(uart.irq_pending());
        assert_eq!(uart.read(2, 1).expect("iir") & 0x0F, 0x04);
    }

    #[test]
    fn rejects_odd_access_sizes_and_full_input() {
        let mut uart = Uart16550::new();
        assert!(uart.read(0, 8).is_err());
        assert!(!uart.host_write(&vec![0_u8; 65 * 1024]));
    }
}
