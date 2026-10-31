//! Platform-Level Interrupt Controller (SiFive PLIC semantics) restricted
//! to a single hart with two contexts, matching our DTB:
//! context 0 = hart 0 M-mode, context 1 = hart 0 S-mode.

use crate::bus::MmioDevice;
use crate::error::{Result, VmError};

/// Number of interrupt sources (1..=MAX_SOURCE). Source 0 is reserved.
pub const MAX_SOURCE: u32 = 32;
/// PLIC context for hart 0 machine mode.
pub const CONTEXT_M: usize = 0;
/// PLIC context for hart 0 supervisor mode.
pub const CONTEXT_S: usize = 1;

const PRIORITY_BASE: u64 = 0x0000;
const PENDING_BASE: u64 = 0x1000;
const ENABLE_BASE: u64 = 0x2000;
const ENABLE_STRIDE: u64 = 0x80;
const CONTEXT_BASE: u64 = 0x20_0000;
const CONTEXT_STRIDE: u64 = 0x1000;

/// Single-hart PLIC.
#[derive(Debug)]
pub struct Plic {
    priority: [u32; (MAX_SOURCE + 1) as usize],
    /// Latched request lines driven by devices (level, recomputed by host).
    lines: u64,
    /// Sources currently claimed by a context (gateway in service).
    in_service: [u64; 2],
    enable: [u64; 2],
    threshold: [u32; 2],
}

impl Default for Plic {
    fn default() -> Self {
        Self::new()
    }
}

impl Plic {
    pub fn new() -> Self {
        Self {
            priority: [0; (MAX_SOURCE + 1) as usize],
            lines: 0,
            in_service: [0; 2],
            enable: [0; 2],
            threshold: [0; 2],
        }
    }

    /// Drives a device interrupt line (level-triggered).
    pub fn set_line(&mut self, source: u32, asserted: bool) {
        if source == 0 || source > MAX_SOURCE {
            return;
        }
        if asserted {
            self.lines |= 1_u64 << source;
        } else {
            self.lines &= !(1_u64 << source);
        }
    }

    fn pending_word(&self) -> u64 {
        let in_service_anywhere = self.in_service[0] | self.in_service[1];
        self.lines & !in_service_anywhere
    }

    /// Whether `context` should see an external interrupt.
    pub fn interrupt_pending(&self, context: usize) -> bool {
        self.best_candidate(context).is_some()
    }

    fn best_candidate(&self, context: usize) -> Option<u32> {
        if context >= 2 {
            return None;
        }
        let candidates = self.pending_word() & self.enable[context];
        let mut best: Option<(u32, u32)> = None;
        for source in 1..=MAX_SOURCE {
            if candidates & (1_u64 << source) == 0 {
                continue;
            }
            let priority = self.priority[source as usize];
            if priority <= self.threshold[context] {
                continue;
            }
            let better = match best {
                None => true,
                Some((best_source, best_priority)) => {
                    priority > best_priority
                        || (priority == best_priority && source < best_source)
                }
            };
            if better {
                best = Some((source, priority));
            }
        }
        best.map(|(source, _)| source)
    }

    fn claim(&mut self, context: usize) -> u32 {
        match self.best_candidate(context) {
            Some(source) => {
                self.in_service[context] |= 1_u64 << source;
                source
            }
            None => 0,
        }
    }

    fn complete(&mut self, context: usize, source: u32) {
        if context < 2 && source <= MAX_SOURCE {
            self.in_service[context] &= !(1_u64 << source);
        }
    }
}

impl MmioDevice for Plic {
    fn read(&mut self, offset: u64, size: usize) -> Result<u64> {
        if size != 4 {
            return Err(VmError::UnsupportedAccessSize { size });
        }
        let value: u32 = match offset {
            o if o < PENDING_BASE => {
                let source = (o / 4) as u32;
                if source <= MAX_SOURCE {
                    self.priority[source as usize]
                } else {
                    0
                }
            }
            o if o < ENABLE_BASE => {
                let word = (o - PENDING_BASE) / 4;
                (self.pending_word() >> (word * 32)) as u32
            }
            o if o < CONTEXT_BASE => {
                let context = ((o - ENABLE_BASE) / ENABLE_STRIDE) as usize;
                let word = ((o - ENABLE_BASE) % ENABLE_STRIDE) / 4;
                if context < 2 {
                    (self.enable[context] >> (word * 32)) as u32
                } else {
                    0
                }
            }
            o => {
                let context = ((o - CONTEXT_BASE) / CONTEXT_STRIDE) as usize;
                let sub = (o - CONTEXT_BASE) % CONTEXT_STRIDE;
                match sub {
                    0 if context < 2 => self.threshold[context],
                    4 if context < 2 => self.claim(context),
                    _ => 0,
                }
            }
        };
        Ok(u64::from(value))
    }

    fn write(&mut self, offset: u64, size: usize, value: u64) -> Result<()> {
        if size != 4 {
            return Err(VmError::UnsupportedAccessSize { size });
        }
        let value = (value & 0xFFFF_FFFF) as u32;
        match offset {
            o if o < PENDING_BASE => {
                let source = (o / 4) as u32;
                if source <= MAX_SOURCE {
                    self.priority[source as usize] = value & 0x7;
                }
            }
            o if o < CONTEXT_BASE && o >= ENABLE_BASE => {
                let context = ((o - ENABLE_BASE) / ENABLE_STRIDE) as usize;
                let word = ((o - ENABLE_BASE) % ENABLE_STRIDE) / 4;
                if context < 2 && word < 2 {
                    let shift = word * 32;
                    self.enable[context] = (self.enable[context] & !(0xFFFF_FFFF_u64 << shift))
                        | (u64::from(value) << shift);
                    // Source 0 is never enableable.
                    self.enable[context] &= !1_u64;
                }
            }
            o if o >= CONTEXT_BASE => {
                let context = ((o - CONTEXT_BASE) / CONTEXT_STRIDE) as usize;
                let sub = (o - CONTEXT_BASE) % CONTEXT_STRIDE;
                if context < 2 {
                    match sub {
                        0 => self.threshold[context] = value & 0x7,
                        4 => self.complete(context, value),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Plic, CONTEXT_S};
    use crate::bus::MmioDevice;

    const CLAIM_S: u64 = 0x20_0000 + 0x1000 + 4;
    const ENABLE_S: u64 = 0x2000 + 0x80;

    fn plic_with_uart_enabled() -> Plic {
        let mut plic = Plic::new();
        plic.write(10 * 4, 4, 5).expect("priority (UART = 5)");
        plic.write(1 * 4, 4, 3).expect("priority (virtio = 3)");
        plic.write(ENABLE_S, 4, (1 << 10) | (1 << 1)).expect("enable");
        plic
    }

    #[test]
    fn claim_picks_highest_priority_then_completes() {
        let mut plic = plic_with_uart_enabled();
        plic.set_line(1, true);
        plic.set_line(10, true);
        assert!(plic.interrupt_pending(CONTEXT_S));
        assert_eq!(plic.read(CLAIM_S, 4).expect("claim"), 10, "UART wins over virtio");
        assert_eq!(plic.read(CLAIM_S, 4).expect("claim 2"), 1);
        plic.write(CLAIM_S, 4, 10).expect("complete 10");
        // Level line still asserted: source 10 becomes claimable again.
        assert_eq!(plic.read(CLAIM_S, 4).expect("claim 3"), 10);
        plic.set_line(10, false);
        plic.set_line(1, false);
        plic.write(CLAIM_S, 4, 1).expect("complete 1");
        plic.write(CLAIM_S, 4, 10).expect("complete 10 again");
        assert!(!plic.interrupt_pending(CONTEXT_S));
        assert_eq!(plic.read(CLAIM_S, 4).expect("empty claim"), 0);
    }

    #[test]
    fn disabled_or_below_threshold_sources_do_not_fire() {
        let mut plic = Plic::new();
        plic.write(10 * 4, 4, 1).expect("priority");
        plic.set_line(10, true);
        assert!(!plic.interrupt_pending(CONTEXT_S), "not enabled");
        plic.write(ENABLE_S, 4, 1 << 10).expect("enable");
        plic.write(0x20_1000, 4, 1).expect("threshold = 1");
        assert!(!plic.interrupt_pending(CONTEXT_S), "priority must exceed threshold");
        plic.write(0x20_1000, 4, 0).expect("threshold = 0");
        assert!(plic.interrupt_pending(CONTEXT_S));
        assert!(plic.read(0, 2).is_err(), "PLIC is 32-bit only");
    }
}
