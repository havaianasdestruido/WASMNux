//! End-to-end Phase 2 integration test booting a real RV64 S-mode mini-OS inside `RiscvMachine`
//! exercising Sv39 MMU, S-mode timer IRQ + WFI + SRET, NS16550A UART, Goldfish RTC, SimpleFB,
//! VirtIO-Blk DMA -> `SparseBlockDisk`, VM lifecycle transitions, and Syscon poweroff.

use wasmnux_riscv::boot::RISCV_IMAGE_MAGIC2;
use wasmnux_riscv::hart::StepOutcome;
use wasmnux_riscv::trap::{AccessType, SystemResetAction};
use wasmnux_riscv::{
    RiscvMachine, VmLifecycleState, GUEST_FB_BASE, GUEST_RAM_BASE, KERNEL_LOAD_OFFSET,
};
use wasmnux_storage::SparseBlockDisk;
use wasmnux_virtio::{
    VirtioBlkMmio, VirtioNetMmio, VirtioRngMmio, VRING_DESC_F_NEXT, VRING_DESC_F_WRITE,
};

const fn addi(rd: u32, rs1: u32, imm: i32) -> u32 {
    (((imm as u32) & 0xFFF) << 20) | (rs1 << 15) | (rd << 7) | 0x13
}
const fn lui(rd: u32, imm20: u32) -> u32 {
    ((imm20 & 0xFFFFF) << 12) | (rd << 7) | 0x37
}
const fn slli(rd: u32, rs1: u32, shamt: u32) -> u32 {
    ((shamt & 0x3F) << 20) | (rs1 << 15) | (1 << 12) | (rd << 7) | 0x13
}
const fn srli(rd: u32, rs1: u32, shamt: u32) -> u32 {
    ((shamt & 0x3F) << 20) | (rs1 << 15) | (5 << 12) | (rd << 7) | 0x13
}
const fn or_r(rd: u32, rs1: u32, rs2: u32) -> u32 {
    (rs2 << 20) | (rs1 << 15) | (6 << 12) | (rd << 7) | 0x33
}
const fn sb(rs2: u32, rs1: u32, imm: i32) -> u32 {
    let u = (imm as u32) & 0xFFF;
    ((u >> 5) << 25) | (rs2 << 20) | (rs1 << 15) | ((u & 0x1F) << 7) | 0x23
}
const fn sw(rs2: u32, rs1: u32, imm: i32) -> u32 {
    let u = (imm as u32) & 0xFFF;
    ((u >> 5) << 25) | (rs2 << 20) | (rs1 << 15) | (2 << 12) | ((u & 0x1F) << 7) | 0x23
}
const fn lw(rd: u32, rs1: u32, imm: i32) -> u32 {
    (((imm as u32) & 0xFFF) << 20) | (rs1 << 15) | (2 << 12) | (rd << 7) | 0x03
}
const fn csrrw(rd: u32, csr: u32, rs1: u32) -> u32 {
    (csr << 20) | (rs1 << 15) | (1 << 12) | (rd << 7) | 0x73
}
const fn csrrs(rd: u32, csr: u32, rs1: u32) -> u32 {
    (csr << 20) | (rs1 << 15) | (2 << 12) | (rd << 7) | 0x73
}

#[test]
fn boots_rv64_smode_mini_os_through_mmu_irq_uart_fb_virtio_blk_and_poweroff() {
    let base_epoch_ns = 1_761_900_000_000_000_000u64;
    let mut vm = RiscvMachine::new(32 * 1024 * 1024, base_epoch_ns);
    assert_eq!(vm.state, VmLifecycleState::Created);
    assert!(vm.pause().is_err());

    let disk = SparseBlockDisk::new(4 * 1024 * 1024).unwrap();
    vm.attach_virtio_mmio(
        0x1000_1000,
        0x1000,
        1,
        Box::new(VirtioBlkMmio::new(1, disk)),
    );
    vm.attach_virtio_mmio(
        0x1000_2000,
        0x1000,
        2,
        Box::new(VirtioNetMmio::new_default(2)),
    );
    vm.attach_virtio_mmio(
        0x1000_3000,
        0x1000,
        3,
        Box::new(VirtioRngMmio::new(3, [7; 16])),
    );

    // Layout inside DRAM after kernel load at 0x8020_0000:
    // - 0x8020_0000: 64-byte RISC-V Linux Image header (`j +64`)
    // - 0x8020_0040: S-mode entry code
    // - 0x8020_0200: S-mode trap vector (`stvec`)
    // - 0x8030_0000: Sv39 root page table (4 KiB aligned)
    // - 0x8031_0000..0x8031_6000: VirtIO-Blk vring & sector payload buffers
    let trap_vec_va = GUEST_RAM_BASE + KERNEL_LOAD_OFFSET + 0x200;
    let root_pt_pa = GUEST_RAM_BASE + 0x0030_0000;
    let vq_desc_pa = GUEST_RAM_BASE + 0x0031_0000;
    let vq_avail_pa = GUEST_RAM_BASE + 0x0031_1000;
    let vq_used_pa = GUEST_RAM_BASE + 0x0031_2000;
    let blk_hdr_pa = GUEST_RAM_BASE + 0x0031_3000;
    let blk_data_pa = GUEST_RAM_BASE + 0x0031_4000;
    let blk_status_pa = GUEST_RAM_BASE + 0x0031_5000;

    let mut image = vec![0u8; 2048];
    let img_len = image.len() as u64;
    image[0..4].copy_from_slice(&0x0400_006Fu32.to_le_bytes()); // j +64
    image[8..16].copy_from_slice(&KERNEL_LOAD_OFFSET.to_le_bytes());
    image[16..24].copy_from_slice(&img_len.to_le_bytes());
    image[56..60].copy_from_slice(&RISCV_IMAGE_MAGIC2.to_le_bytes());

    // Pre-computed satp = (8 << 60) | (root_pt_pa >> 12) = 0x8000_0000_0008_0300
    // Build S-mode entry sequence at offset 0x40:
    let main_prog: &[u32] = &[
        // 1. Activate Sv39 MMU (`satp` = (8 << 60) | (0x80300000 >> 12)) + SFENCE.VMA
        addi(5, 0, 8),
        slli(5, 5, 60),
        lui(6, 0x80),
        addi(6, 6, 0x300), // x6 = 0x80300
        or_r(5, 5, 6),
        csrrw(0, 0x180, 5), // csrw satp, x5
        0x1200_0073,        // sfence.vma
        // 2. Program stvec = 0x8020_0200, enable sie.STIE (1<<5) and sstatus.SIE (1<<1)
        // Load 0x80200200 without sign-extension pitfall: (0x80200 << 12) & 0xFFFF_FFFF + 0x200
        lui(7, 0x80200),
        slli(7, 7, 32),
        srli(7, 7, 32),
        addi(7, 7, 0x200),
        csrrw(0, 0x105, 7), // csrw stvec, x7
        addi(8, 0, 1 << 5),
        csrrs(0, 0x104, 8), // csrs sie, STIE
        addi(8, 0, 1 << 1),
        csrrs(0, 0x100, 8), // csrs sstatus, SIE
        // 3. Arm timer via SBI legacy SET_TIMER (eid=0, a0=25) and execute WFI!
        addi(10, 0, 25),
        addi(17, 0, 0),
        0x0000_0073, // ecall (set_timer)
        0x1050_0073, // wfi -> fast-forwards mtime to 25 and traps to 0x8020_0200!
        // 4. Back from SRET! Write 'O', 'K' to NS16550A UART (0x1000_0000)
        lui(9, 0x10000),
        addi(8, 0, b'O' as i32),
        sb(8, 9, 0),
        addi(8, 0, b'K' as i32),
        sb(8, 9, 0),
        // 5. Read Goldfish RTC low word at 0x0010_1000 into x18 (must be non-zero)
        lui(9, 0x101),
        lw(18, 9, 0),
        // 6. Paint pixel in SimpleFB row 20 (0x5000_0000 + 20 * 2560 = 0x5000_C800)
        lui(9, 0x5000D),
        addi(9, 9, -0x800), // 0x5000_C800 = row 20, col 0
        lui(8, 0x00FF8),
        sw(8, 9, 0),
        // 7. Notify VirtIO-Blk (0x1000_1000) QueueNotify(0) at offset 0x050
        lui(9, 0x10001),
        sw(0, 9, 0x50),
        // 8. Request clean hardware PowerOff via Syscon (write 0x5555 to 0x0010_0000)
        lui(9, 0x100),
        lui(8, 0x5),
        addi(8, 8, 0x555),
        sw(8, 9, 0),
    ];
    for (i, &insn) in main_prog.iter().enumerate() {
        let off = 0x40 + i * 4;
        image[off..off + 4].copy_from_slice(&insn.to_le_bytes());
    }

    // S-mode trap handler at offset 0x200 (`trap_vec_va` = 0x8020_0200):
    // Disarm timer (SBI set_timer = u64::MAX), write 'T' to UART 0x1000_0000, and SRET!
    let trap_prog: &[u32] = &[
        addi(10, 0, -1),  // a0 = u64::MAX
        addi(17, 0, 0),   // a7 = 0 (SBI legacy set_timer)
        0x0000_0073,      // ecall
        lui(28, 0x10000), // t3 = 0x1000_0000 (UART)
        addi(29, 0, b'T' as i32),
        sb(29, 28, 0), // write 'T'
        0x1020_0073,   // sret
    ];
    for (i, &insn) in trap_prog.iter().enumerate() {
        let off = 0x200 + i * 4;
        image[off..off + 4].copy_from_slice(&insn.to_le_bytes());
    }
    let _ = trap_vec_va;

    vm.load_linux_image(&image, None, "console=ttyS0", [0x11; 32])
        .expect("load mini-OS image");
    assert_eq!(vm.state, VmLifecycleState::Running);

    // Populate Sv39 identity-mapped 1 GiB gigapages at `root_pt_pa`:
    // VPN[2]=0 (0x0000_0000), VPN[2]=1 (0x4000_0000), VPN[2]=2 (0x8000_0000)
    let flags_rwx = (1 << 7) | (1 << 6) | (1 << 3) | (1 << 2) | (1 << 1) | 1;
    for vpn2 in 0..3u64 {
        let ppn = (vpn2 << 30) >> 12;
        let pte = (ppn << 10) | flags_rwx;
        vm.bus
            .ram
            .write_scalar(root_pt_pa + vpn2 * 8, 8, pte, AccessType::Store)
            .unwrap();
    }

    // Pre-configure VirtIO-Blk queue 0 descriptors in RAM so the guest's QueueNotify(0) DMAs sector 0!
    let mut blk_hdr = [0u8; 16];
    blk_hdr[0..4].copy_from_slice(&1u32.to_le_bytes()); // VIRTIO_BLK_T_OUT
    blk_hdr[8..16].copy_from_slice(&0u64.to_le_bytes()); // sector 0
    vm.bus.ram.write_bytes(blk_hdr_pa, &blk_hdr).unwrap();

    let mut sector0 = [0u8; 512];
    let tag = b"MINI-OS-VIRTIO-BLK-OK";
    sector0[..tag.len()].copy_from_slice(tag);
    vm.bus.ram.write_bytes(blk_data_pa, &sector0).unwrap();
    vm.bus.ram.write_bytes(blk_status_pa, &[0xFF]).unwrap();

    let put_desc = |ram: &mut wasmnux_riscv::bus::GuestRam,
                    idx: u16,
                    addr: u64,
                    len: u32,
                    flags: u16,
                    next: u16| {
        let mut d = [0u8; 16];
        d[0..8].copy_from_slice(&addr.to_le_bytes());
        d[8..12].copy_from_slice(&len.to_le_bytes());
        d[12..14].copy_from_slice(&flags.to_le_bytes());
        d[14..16].copy_from_slice(&next.to_le_bytes());
        ram.write_bytes(vq_desc_pa + u64::from(idx) * 16, &d)
            .unwrap();
    };
    put_desc(&mut vm.bus.ram, 0, blk_hdr_pa, 16, VRING_DESC_F_NEXT, 1);
    put_desc(&mut vm.bus.ram, 1, blk_data_pa, 512, VRING_DESC_F_NEXT, 2);
    put_desc(&mut vm.bus.ram, 2, blk_status_pa, 1, VRING_DESC_F_WRITE, 0);
    vm.bus
        .ram
        .write_scalar(vq_avail_pa + 2, 2, 1, AccessType::Store)
        .unwrap();
    vm.bus
        .ram
        .write_scalar(vq_avail_pa + 4, 2, 0, AccessType::Store)
        .unwrap();

    // Configure VirtIO-Blk MMIO queue addresses
    for &(off, val) in &[
        (0x030u64, 0u64),
        (0x038, 16),
        (0x080, vq_desc_pa & 0xFFFF_FFFF),
        (0x084, vq_desc_pa >> 32),
        (0x090, vq_avail_pa & 0xFFFF_FFFF),
        (0x094, vq_avail_pa >> 32),
        (0x0A0, vq_used_pa & 0xFFFF_FFFF),
        (0x0A4, vq_used_pa >> 32),
        (0x044, 1),
    ] {
        vm.bus
            .write_phys(0x1000_1000 + off, 4, val, AccessType::Store)
            .unwrap();
    }
    let _ = vm.bus.fb.take_dirty_range(); // clear initial dirty state

    // Exercise pause / resume lifecycle before running to completion
    vm.pause().unwrap();
    assert_eq!(vm.step_slice(10).0, 0);
    vm.resume().unwrap();

    let (_, outcome) = vm.step_slice(200);
    assert_eq!(outcome, StepOutcome::Reset(SystemResetAction::PowerOff));
    assert_eq!(vm.state, VmLifecycleState::PoweredOff);

    // 1. Timer interrupt ('T') fired BEFORE main thread continued ('O', 'K')!
    assert_eq!(vm.bus.uart.drain_output(), b"TOK");
    // 2. Goldfish RTC returned non-zero epoch low bits
    assert_ne!(vm.hart.regs[18], 0);
    // 3. SimpleFB tracked exact dirty scanline 20..21 at GUEST_FB_BASE
    assert_eq!(vm.bus.fb.take_dirty_range(), Some((20, 21)));
    assert_ne!(
        vm.bus
            .read_phys(GUEST_FB_BASE + 20 * 640 * 4, 4, AccessType::Load)
            .unwrap(),
        0
    );
    // 4. VirtIO-Blk DMA completed with status == 0 (`VIRTIO_BLK_S_OK`)
    assert_eq!(
        vm.bus
            .ram
            .read_scalar(blk_status_pa, 1, AccessType::Load)
            .unwrap(),
        0
    );
}
