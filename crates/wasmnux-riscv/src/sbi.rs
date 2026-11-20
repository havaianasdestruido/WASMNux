//! RISC-V Supervisor Binary Interface (`SBI v2.0`) handler for S-mode `ecall` traps.

use crate::bus::{MachineBus, MAX_HARTS};
use crate::trap::SystemResetAction;

const SBI_EXT_LEGACY_SET_TIMER: u64 = 0x00;
const SBI_EXT_LEGACY_CONSOLE_PUTCHAR: u64 = 0x01;
const SBI_EXT_LEGACY_CONSOLE_GETCHAR: u64 = 0x02;
const SBI_EXT_LEGACY_CLEAR_IPI: u64 = 0x03;
const SBI_EXT_LEGACY_SEND_IPI: u64 = 0x04;
const SBI_EXT_LEGACY_REMOTE_FENCE_I: u64 = 0x05;
const SBI_EXT_LEGACY_REMOTE_SFENCE_VMA: u64 = 0x06;
const SBI_EXT_LEGACY_REMOTE_SFENCE_VMA_ASID: u64 = 0x07;
const SBI_EXT_LEGACY_SHUTDOWN: u64 = 0x08;

const SBI_EXT_BASE: u64 = 0x10;
const SBI_EXT_TIME: u64 = 0x5449_4D45; // "TIME"
const SBI_EXT_IPI: u64 = 0x0073_5049; // "sPI"
const SBI_EXT_RFENCE: u64 = 0x5246_4E43; // "RFNC"
const SBI_EXT_HSM: u64 = 0x0048_534D; // "HSM"
const SBI_EXT_SRST: u64 = 0x5352_5354; // "SRST"
const SBI_EXT_DBCN: u64 = 0x4442_434E; // "DBCN"

const SBI_SUCCESS: i64 = 0;
const SBI_ERR_NOT_SUPPORTED: i64 = -2;
const SBI_ERR_INVALID_PARAM: i64 = -3;
const SBI_ERR_ALREADY_STARTED: i64 = -6;

/// Architectural HSM hart states reported by `sbi_hart_get_status` (SBI v1.0 §HSM).
pub const HSM_STATE_STARTED: u64 = 0;
pub const HSM_STATE_STOPPED: u64 = 1;
pub const HSM_STATE_START_PENDING: u64 = 2;
pub const HSM_STATE_STOP_PENDING: u64 = 3;

#[derive(Clone, Debug, Default)]
pub struct VirtualSbi {
    pub reset_action: Option<SystemResetAction>,
}

impl VirtualSbi {
    pub fn new() -> Self {
        Self { reset_action: None }
    }

    /// Handle an S-mode `ecall` issued by hart `hart_id`. Updates `regs[10]` (`a0` error) and
    /// `regs[11]` (`a1` value). Returns `true` if TLB/ICache should be flushed.
    ///
    /// `machine_hart` is used for the RFENCE/hart-mask helpers and `sip_ssip_set` carries a
    /// supervisor software interrupt injection request to the caller (hart mask + level).
    pub fn handle_ecall(
        &mut self,
        regs: &mut [u64; 32],
        bus: &mut MachineBus,
        sip_ssip_set: &mut Option<(u64, bool)>,
    ) -> bool {
        let hart_id = bus.current_hart;
        let a0 = regs[10];
        let a1 = regs[11];
        let a2 = regs[12];
        let fid = regs[16];
        let eid = regs[17];

        let mut flush_caches = false;

        match eid {
            SBI_EXT_LEGACY_SET_TIMER => {
                bus.clint.mtimecmp[hart_id.min(MAX_HARTS - 1)] = a0;
                regs[10] = 0;
            }
            SBI_EXT_LEGACY_CONSOLE_PUTCHAR => {
                bus.uart.push_console_output(a0 as u8);
                regs[10] = 0;
            }
            SBI_EXT_LEGACY_CONSOLE_GETCHAR => {
                regs[10] = bus.uart.pop_input().map_or((-1i64) as u64, u64::from);
            }
            SBI_EXT_LEGACY_CLEAR_IPI => {
                *sip_ssip_set = Some((u64::MAX, false));
                regs[10] = 0;
            }
            SBI_EXT_LEGACY_SEND_IPI => {
                *sip_ssip_set = Some((u64::MAX, true));
                regs[10] = 0;
            }
            SBI_EXT_LEGACY_REMOTE_FENCE_I
            | SBI_EXT_LEGACY_REMOTE_SFENCE_VMA
            | SBI_EXT_LEGACY_REMOTE_SFENCE_VMA_ASID => {
                flush_caches = true;
                regs[10] = 0;
            }
            SBI_EXT_LEGACY_SHUTDOWN => {
                self.reset_action = Some(SystemResetAction::PowerOff);
                regs[10] = 0;
            }
            SBI_EXT_BASE => {
                let (err, val): (i64, u64) = match fid {
                    0 => (SBI_SUCCESS, (2 << 24)),   // SBI Spec v2.0
                    1 => (SBI_SUCCESS, 0x574E_5558), // Impl ID "WNUX"
                    2 => (SBI_SUCCESS, 1),
                    3 => {
                        let supported = matches!(
                            a0,
                            SBI_EXT_BASE
                                | SBI_EXT_TIME
                                | SBI_EXT_IPI
                                | SBI_EXT_RFENCE
                                | SBI_EXT_HSM
                                | SBI_EXT_SRST
                                | SBI_EXT_DBCN
                                | SBI_EXT_LEGACY_SET_TIMER
                                | SBI_EXT_LEGACY_CONSOLE_PUTCHAR
                        );
                        (SBI_SUCCESS, u64::from(supported))
                    }
                    4..=6 => (SBI_SUCCESS, 0), // mvendorid / marchid / mimpid
                    _ => (SBI_ERR_NOT_SUPPORTED, 0),
                };
                regs[10] = err as u64;
                regs[11] = val;
            }
            SBI_EXT_TIME => {
                if fid == 0 {
                    bus.clint.mtimecmp[hart_id.min(MAX_HARTS - 1)] = a0;
                    regs[10] = SBI_SUCCESS as u64;
                    regs[11] = 0;
                } else {
                    regs[10] = SBI_ERR_NOT_SUPPORTED as u64;
                    regs[11] = 0;
                }
            }
            SBI_EXT_IPI => {
                if fid == 0 {
                    // sbi_send_ipi(hart_mask = a0)
                    *sip_ssip_set = Some((a0, true));
                    regs[10] = SBI_SUCCESS as u64;
                    regs[11] = 0;
                } else {
                    regs[10] = SBI_ERR_NOT_SUPPORTED as u64;
                    regs[11] = 0;
                }
            }
            SBI_EXT_RFENCE => {
                // sbi_remote_fence_i / sfence_vma / sfence_vma_asid: all harts share the
                // emulator\'s single address-space cache, so a local flush is sufficient.
                match fid {
                    0..=2 => {
                        flush_caches = true;
                        regs[10] = SBI_SUCCESS as u64;
                        regs[11] = 0;
                    }
                    _ => {
                        regs[10] = SBI_ERR_NOT_SUPPORTED as u64;
                        regs[11] = 0;
                    }
                }
            }
            SBI_EXT_HSM => {
                match fid {
                    // sbi_hart_start(hartid = a0, start_addr = a1, opaque = a2)
                    0 => {
                        let target = a0 as usize;
                        if target >= bus.hart_count.min(MAX_HARTS) {
                            regs[10] = SBI_ERR_INVALID_PARAM as u64;
                        } else {
                            let already_running =
                                bus.hsm_requests.get(target).is_some_and(|r| r.started)
                                    && !bus.hart_halt.get(target).copied().unwrap_or(false);
                            if already_running {
                                regs[10] = SBI_ERR_ALREADY_STARTED as u64;
                            } else {
                                if let Some(req) =
                                    bus.hsm_requests.iter_mut().find(|r| r.hart_id == target)
                                {
                                    req.start_request = Some((a1, a2));
                                    req.started = false;
                                }
                                bus.hart_halt[target] = false;
                                bus.hart_scratch_targets[target] = Some(a2);
                                regs[10] = SBI_SUCCESS as u64;
                            }
                        }
                        regs[11] = 0;
                    }
                    // sbi_hart_stop()
                    1 => {
                        bus.hart_halt[hart_id.min(MAX_HARTS - 1)] = true;
                        regs[10] = SBI_SUCCESS as u64;
                        regs[11] = 0;
                    }
                    // sbi_hart_get_status(hartid = a0)
                    2 => {
                        let target = a0 as usize;
                        if target < bus.hart_count.min(MAX_HARTS) {
                            let status = if bus.hart_halt[target] {
                                HSM_STATE_STOPPED
                            } else {
                                HSM_STATE_STARTED
                            };
                            regs[10] = SBI_SUCCESS as u64;
                            regs[11] = status;
                        } else {
                            regs[10] = SBI_ERR_INVALID_PARAM as u64;
                            regs[11] = 0;
                        }
                    }
                    // sbi_hart_suspend(...)
                    3 => {
                        regs[10] = SBI_ERR_NOT_SUPPORTED as u64;
                        regs[11] = 0;
                    }
                    _ => {
                        regs[10] = SBI_ERR_NOT_SUPPORTED as u64;
                        regs[11] = 0;
                    }
                }
            }
            SBI_EXT_SRST => {
                if fid == 0 {
                    match a0 {
                        0 => {
                            self.reset_action = Some(SystemResetAction::PowerOff);
                            regs[10] = SBI_SUCCESS as u64;
                            regs[11] = 0;
                        }
                        1 => {
                            self.reset_action = Some(SystemResetAction::ColdReboot);
                            regs[10] = SBI_SUCCESS as u64;
                            regs[11] = 0;
                        }
                        2 => {
                            self.reset_action = Some(SystemResetAction::WarmReboot);
                            regs[10] = SBI_SUCCESS as u64;
                            regs[11] = 0;
                        }
                        _ => {
                            regs[10] = SBI_ERR_INVALID_PARAM as u64;
                            regs[11] = 0;
                        }
                    }
                } else {
                    regs[10] = SBI_ERR_NOT_SUPPORTED as u64;
                    regs[11] = 0;
                }
            }
            SBI_EXT_DBCN => {
                match fid {
                    0 => {
                        // sbi_debug_console_write(num_bytes = a0, base_addr_lo = a1, base_addr_hi = a2)
                        let _ = a2;
                        let len = (a0 as usize).min(65536);
                        let mut written = 0usize;
                        for i in 0..len {
                            if let Ok(b) = bus.ram.read_scalar(
                                a1.wrapping_add(i as u64),
                                1,
                                crate::trap::AccessType::Load,
                            ) {
                                bus.uart.push_console_output(b as u8);
                                written += 1;
                            } else {
                                break;
                            }
                        }
                        regs[10] = SBI_SUCCESS as u64;
                        regs[11] = written as u64;
                    }
                    2 => {
                        // sbi_debug_console_write_byte(byte = a0)
                        bus.uart.push_console_output(a0 as u8);
                        regs[10] = SBI_SUCCESS as u64;
                        regs[11] = 0;
                    }
                    _ => {
                        regs[10] = SBI_ERR_NOT_SUPPORTED as u64;
                        regs[11] = 0;
                    }
                }
            }
            _ => {
                regs[10] = SBI_ERR_NOT_SUPPORTED as u64;
                regs[11] = 0;
            }
        }
        flush_caches
    }
}

/// Per-hart HSM (SBI Hart State Management) bookkeeping shared between `RiscvMachine` and `Hart`.
#[derive(Clone, Debug, Default)]
pub struct HartHsmState {
    pub hart_id: usize,
    pub started: bool,
    pub start_request: Option<(u64, u64)>,
}
