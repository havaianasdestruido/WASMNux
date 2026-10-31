//! System bus: the physical address map (QEMU `virt`-compatible, see
//! `docs/ARCHITECTURE.md` §2). Unmapped accesses return an error which the
//! CPU converts into a guest access-fault trap — never a host panic.

use crate::block::VecBlockBackend;
use crate::clint::Clint;
use crate::error::{Result, VmError};
use crate::memory::Ram;
use crate::plic::Plic;
use crate::uart::Uart16550;
use crate::virtio_blk::VirtioBlock;

/// Base of guest RAM.
pub const RAM_BASE: u64 = 0x8000_0000;
/// CLINT base / size.
pub const CLINT_BASE: u64 = 0x0200_0000;
pub const CLINT_SIZE: u64 = 0x0001_0000;
/// PLIC base / size as advertised in the DTB.
pub const PLIC_BASE: u64 = 0x0C00_0000;
pub const PLIC_SIZE: u64 = 0x0400_0000;
/// UART0 base / size.
pub const UART_BASE: u64 = 0x1000_0000;
pub const UART_SIZE: u64 = 0x100;
/// First VirtIO-MMIO slot base / size.
pub const VIRTIO_BASE: u64 = 0x1000_1000;
pub const VIRTIO_SIZE: u64 = 0x1000;
/// PLIC source for the VirtIO block device.
pub const IRQ_VIRTIO: u32 = 1;
/// PLIC source for UART0.
pub const IRQ_UART: u32 = 10;
/// Kernel `Image` load offset from RAM base (RISC-V boot protocol).
pub const KERNEL_OFFSET: u64 = 0x0020_0000;

/// An MMIO device on the system bus.
pub trait MmioDevice {
    /// Reads `size` bytes at device-relative `offset`.
    ///
    /// # Errors
    /// [`VmError::UnsupportedAccessSize`] for sizes the device rejects.
    fn read(&mut self, offset: u64, size: usize) -> Result<u64>;

    /// Writes the low `size` bytes of `value` at device-relative `offset`.
    ///
    /// # Errors
    /// [`VmError::UnsupportedAccessSize`] for sizes the device rejects.
    fn write(&mut self, offset: u64, size: usize, value: u64) -> Result<()>;
}

/// The physical address space of the virtual machine.
#[derive(Debug)]
pub struct SystemBus {
    ram: Ram,
    clint: Clint,
    plic: Plic,
    uart: Uart16550,
    virtio_blk: Option<VirtioBlock<VecBlockBackend>>,
}

impl SystemBus {
    /// Builds a bus with `ram_size` bytes of RAM and no disk attached.
    ///
    /// # Errors
    /// Propagates [`Ram::new`] failures.
    pub fn new(ram_size: usize) -> Result<Self> {
        Ok(Self {
            ram: Ram::new(RAM_BASE, ram_size)?,
            clint: Clint::new(),
            plic: Plic::new(),
            uart: Uart16550::new(),
            virtio_blk: None,
        })
    }

    /// Attaches a disk image as the VirtIO block device.
    pub fn attach_disk(&mut self, backend: VecBlockBackend) {
        self.virtio_blk = Some(VirtioBlock::new(backend));
    }

    pub fn has_disk(&self) -> bool {
        self.virtio_blk.is_some()
    }

    /// Access to the attached disk backend (host import/export path).
    pub fn disk(&self) -> Option<&VecBlockBackend> {
        self.virtio_blk.as_ref().map(VirtioBlock::backend)
    }

    pub fn disk_mut(&mut self) -> Option<&mut VecBlockBackend> {
        self.virtio_blk.as_mut().map(VirtioBlock::backend_mut)
    }

    pub fn ram(&self) -> &Ram {
        &self.ram
    }

    pub fn ram_mut(&mut self) -> &mut Ram {
        &mut self.ram
    }

    pub fn clint_mut(&mut self) -> &mut Clint {
        &mut self.clint
    }

    pub fn clint(&self) -> &Clint {
        &self.clint
    }

    pub fn uart_mut(&mut self) -> &mut Uart16550 {
        &mut self.uart
    }

    pub fn plic(&self) -> &Plic {
        &self.plic
    }

    fn in_range(addr: u64, base: u64, size: u64) -> bool {
        addr >= base && addr < base + size
    }

    /// Reads an integer of `size` bytes from the physical address space.
    ///
    /// # Errors
    /// [`VmError::OutOfBounds`] for unmapped addresses (the CPU turns this
    /// into a guest access-fault), or device access errors.
    pub fn read_phys(&mut self, addr: u64, size: usize) -> Result<u64> {
        if self.ram.contains(addr, size) {
            return self.ram.read_int(addr, size);
        }
        if Self::in_range(addr, CLINT_BASE, CLINT_SIZE) {
            return self.clint.read(addr - CLINT_BASE, size);
        }
        if Self::in_range(addr, PLIC_BASE, PLIC_SIZE) {
            return self.plic.read(addr - PLIC_BASE, size);
        }
        if Self::in_range(addr, UART_BASE, UART_SIZE) {
            return self.uart.read(addr - UART_BASE, size);
        }
        if Self::in_range(addr, VIRTIO_BASE, VIRTIO_SIZE) {
            if let Some(dev) = self.virtio_blk.as_mut() {
                return dev.read(addr - VIRTIO_BASE, size);
            }
        }
        Err(VmError::OutOfBounds { addr, len: size })
    }

    /// Writes an integer of `size` bytes to the physical address space,
    /// processing a VirtIO queue kick synchronously before returning.
    ///
    /// # Errors
    /// [`VmError::OutOfBounds`] for unmapped addresses, or device errors.
    pub fn write_phys(&mut self, addr: u64, size: usize, value: u64) -> Result<()> {
        if self.ram.contains(addr, size) {
            return self.ram.write_int(addr, size, value);
        }
        if Self::in_range(addr, CLINT_BASE, CLINT_SIZE) {
            return self.clint.write(addr - CLINT_BASE, size, value);
        }
        if Self::in_range(addr, PLIC_BASE, PLIC_SIZE) {
            return self.plic.write(addr - PLIC_BASE, size, value);
        }
        if Self::in_range(addr, UART_BASE, UART_SIZE) {
            return self.uart.write(addr - UART_BASE, size, value);
        }
        if Self::in_range(addr, VIRTIO_BASE, VIRTIO_SIZE) {
            if let Some(dev) = self.virtio_blk.as_mut() {
                dev.write(addr - VIRTIO_BASE, size, value)?;
                if dev.take_notify_pending() {
                    // The device needs RAM; process with split borrows.
                    let mut dev = self
                        .virtio_blk
                        .take()
                        .expect("virtio device checked present above");
                    dev.process_queue(&mut self.ram);
                    self.virtio_blk = Some(dev);
                }
                return Ok(());
            }
        }
        Err(VmError::OutOfBounds { addr, len: size })
    }

    /// Byte-slice read fast path used by the CPU for physical accesses
    /// (page-table walks and instruction fetch go through here too).
    ///
    /// # Errors
    /// As [`Self::read_phys`].
    pub fn read_phys_bytes(&mut self, addr: u64, buf: &mut [u8]) -> Result<()> {
        if self.ram.contains(addr, buf.len()) {
            return self.ram.read(addr, buf);
        }
        // Devices only support whole registers; assemble byte-wise for the
        // (rare) odd sizes the CPU requests after checking size legality.
        if buf.len() == 1 || buf.len() == 2 || buf.len() == 4 || buf.len() == 8 {
            let value = self.read_phys(addr, buf.len())?;
            buf.copy_from_slice(&value.to_le_bytes()[..buf.len()]);
            return Ok(());
        }
        Err(VmError::UnsupportedAccessSize { size: buf.len() })
    }

    /// Refreshes PLIC input lines from device state. Called by the machine
    /// around execution slices; keeps level-triggered semantics honest.
    pub fn sync_interrupts(&mut self) {
        let uart_irq = self.uart.irq_pending();
        let virtio_irq = self
            .virtio_blk
            .as_ref()
            .is_some_and(VirtioBlock::irq_pending);
        self.plic.set_line(IRQ_UART, uart_irq);
        self.plic.set_line(IRQ_VIRTIO, virtio_irq);
    }
}

#[cfg(test)]
mod tests {
    use super::{SystemBus, CLINT_BASE, RAM_BASE, UART_BASE};

    #[test]
    fn address_decoding_hits_the_right_device() {
        let mut bus = SystemBus::new(1024 * 1024).expect("bus");
        bus.write_phys(RAM_BASE + 16, 4, 0xCAFE).expect("ram write");
        assert_eq!(bus.read_phys(RAM_BASE + 16, 4).expect("ram read"), 0xCAFE);
        // UART at its MMIO home, not in RAM.
        bus.write_phys(UART_BASE, 1, u64::from(b'!'))
            .expect("uart write");
        assert_eq!(bus.uart_mut().take_output(), b"!".to_vec());
        // CLINT mtime starts at zero.
        assert_eq!(bus.read_phys(CLINT_BASE + 0xBFF8, 8).expect("mtime"), 0);
    }

    #[test]
    fn unmapped_access_is_an_error_not_a_panic() {
        let mut bus = SystemBus::new(1024 * 1024).expect("bus");
        assert!(bus.read_phys(0x3000_0000, 4).is_err());
        assert!(bus.write_phys(0x3000_0000, 4, 0).is_err());
        assert!(bus.read_phys(0, 8).is_err());
    }
}
