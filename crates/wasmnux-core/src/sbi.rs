//! Built-in minimal SBI shim (development boot path). When no OpenSBI
//! firmware image is loaded, the machine can service S-mode `ecall`s
//! itself — the same architectural role BBL/OpenSBI play in M-mode, but
//! implemented in the emulator. This is **firmware**, not a kernel patch:
//! guest code is unmodified either way. Implemented: BASE, TIME, SRST and
//! the legacy `set_timer` / console putchar/getchar / shutdown calls.

use crate::bus::SystemBus;
use crate::cpu::Cpu;

const EID_BASE: u64 = 0x10;
const EID_TIME: u64 = 0x5449_4D45; // "TIME"
const EID_SRST: u64 = 0x5352_5354; // "SRST"
const EID_LEGACY_SET_TIMER: u64 = 0x00;
const EID_LEGACY_PUTCHAR: u64 = 0x01;
const EID_LEGACY_GETCHAR: u64 = 0x02;
const EID_LEGACY_SHUTDOWN: u64 = 0x08;

const SBI_SUCCESS: u64 = 0;
const SBI_ERR_NOT_SUPPORTED: u64 = (-2_i64) as u64;
const SBI_ERR_FAILURE: u64 = (-1_i64) as u64;

/// What the machine should do after an SBI call completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SbiOutcome {
    Continue,
    Shutdown,
    Reboot,
}

/// Services one S-mode `ecall`. Registers follow the SBI calling
/// convention: `a7` = EID, `a6` = FID, `a0..a5` arguments;
/// returns `a0` = error, `a1` = value.
pub fn handle_sbi_call(cpu: &mut Cpu, bus: &mut SystemBus) -> SbiOutcome {
    let eid = cpu.reg(17);
    let fid = cpu.reg(16);
    let arg0 = cpu.reg(10);
    let mut error = SBI_SUCCESS;
    let mut value = 0_u64;
    let mut outcome = SbiOutcome::Continue;

    match eid {
        EID_BASE => match fid {
            0 => value = 0x0200_0000, // SBI specification v2.0
            1 => value = 0x574E_5558, // implementation id "WNUX"
            2 => value = 1,           // implementation version
            3 => {
                value = u64::from(matches!(
                    arg0,
                    EID_BASE
                        | EID_TIME
                        | EID_SRST
                        | EID_LEGACY_SET_TIMER
                        | EID_LEGACY_PUTCHAR
                        | EID_LEGACY_GETCHAR
                        | EID_LEGACY_SHUTDOWN
                ));
            }
            4 => value = 0, // mvendorid (matches CSR)
            5 => value = 0, // marchid
            6 => value = 0, // mimpid
            _ => error = SBI_ERR_NOT_SUPPORTED,
        },
        EID_TIME | EID_LEGACY_SET_TIMER => {
            bus.clint_mut().set_mtimecmp(arg0);
            cpu.set_supervisor_timer_pending(false);
        }
        EID_SRST => {
            // reset_type: 0 = shutdown, 1 = cold reboot, 2 = warm reboot.
            match arg0 {
                0 => outcome = SbiOutcome::Shutdown,
                1 | 2 => outcome = SbiOutcome::Reboot,
                _ => error = SBI_ERR_NOT_SUPPORTED,
            }
        }
        EID_LEGACY_PUTCHAR => {
            bus.uart_mut().host_push_output((arg0 & 0xFF) as u8);
        }
        EID_LEGACY_GETCHAR => {
            match bus.uart_mut().host_pop_input() {
                Some(byte) => value = u64::from(byte),
                None => error = SBI_ERR_FAILURE,
            }
            // Legacy convention: result comes back in a0 itself.
            cpu.set_reg(10, if error == SBI_SUCCESS { value } else { error });
            cpu.set_reg(11, 0);
            return outcome;
        }
        EID_LEGACY_SHUTDOWN => outcome = SbiOutcome::Shutdown,
        _ => error = SBI_ERR_NOT_SUPPORTED,
    }

    cpu.set_reg(10, error);
    cpu.set_reg(11, value);
    outcome
}

#[cfg(test)]
mod tests {
    use super::{handle_sbi_call, SbiOutcome};
    use crate::bus::{SystemBus, RAM_BASE};
    use crate::cpu::Cpu;

    fn pair() -> (Cpu, SystemBus) {
        (Cpu::new(RAM_BASE), SystemBus::new(1 << 20).expect("bus"))
    }

    #[test]
    fn base_probe_reports_implemented_extensions_only() {
        let (mut cpu, mut bus) = pair();
        cpu.set_reg(17, 0x10); // BASE
        cpu.set_reg(16, 3); // probe_extension
        cpu.set_reg(10, 0x5449_4D45); // TIME
        assert_eq!(handle_sbi_call(&mut cpu, &mut bus), SbiOutcome::Continue);
        assert_eq!(cpu.reg(10), 0);
        assert_eq!(cpu.reg(11), 1);

        cpu.set_reg(17, 0x10);
        cpu.set_reg(16, 3);
        cpu.set_reg(10, 0xDEAD);
        handle_sbi_call(&mut cpu, &mut bus);
        assert_eq!(cpu.reg(11), 0, "unimplemented extension must probe as 0");
    }

    #[test]
    fn putchar_and_timer_and_shutdown() {
        let (mut cpu, mut bus) = pair();
        cpu.set_reg(17, 0x01);
        cpu.set_reg(10, u64::from(b'K'));
        handle_sbi_call(&mut cpu, &mut bus);
        assert_eq!(bus.uart_mut().take_output(), b"K".to_vec());

        cpu.set_reg(17, 0x5449_4D45);
        cpu.set_reg(16, 0);
        cpu.set_reg(10, 12345);
        handle_sbi_call(&mut cpu, &mut bus);
        assert_eq!(bus.clint().mtimecmp(), 12345);

        cpu.set_reg(17, 0x08);
        assert_eq!(handle_sbi_call(&mut cpu, &mut bus), SbiOutcome::Shutdown);
    }
}
