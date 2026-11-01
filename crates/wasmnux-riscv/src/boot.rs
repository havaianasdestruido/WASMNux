//! RISC-V Linux `Image` boot protocol header parser (`RSC\x05`) and bare-metal ELF64 loader.

use crate::KERNEL_LOAD_OFFSET;

/// Official RISC-V Linux `Image` magic (`"RSC\x05"` in little-endian at byte offset `0x38`).
pub const RISCV_IMAGE_MAGIC2: u32 = 0x0543_5352;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiscvLinuxImageHeader {
    pub text_offset: u64,
    pub image_size: u64,
    pub flags: u64,
    pub version: u32,
}

impl RiscvLinuxImageHeader {
    pub fn parse(image: &[u8]) -> Result<Self, &'static str> {
        if image.len() < 64 {
            return Err("kernel image shorter than 64-byte RISC-V Image header");
        }
        let magic2 = u32::from_le_bytes([image[56], image[57], image[58], image[59]]);
        if magic2 != RISCV_IMAGE_MAGIC2 {
            return Err("invalid RISC-V Linux Image header magic (expected RSC\\x05 at 0x38)");
        }
        let text_offset = u64::from_le_bytes([
            image[8], image[9], image[10], image[11], image[12], image[13], image[14], image[15],
        ]);
        let image_size = u64::from_le_bytes([
            image[16], image[17], image[18], image[19], image[20], image[21], image[22], image[23],
        ]);
        let flags = u64::from_le_bytes([
            image[24], image[25], image[26], image[27], image[28], image[29], image[30], image[31],
        ]);
        let version = u32::from_le_bytes([image[32], image[33], image[34], image[35]]);
        let effective_offset = if text_offset == 0 {
            KERNEL_LOAD_OFFSET
        } else {
            text_offset
        };
        Ok(Self {
            text_offset: effective_offset,
            image_size: image_size.max(image.len() as u64),
            flags,
            version,
        })
    }
}
