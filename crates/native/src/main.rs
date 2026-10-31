//! Native reference harness. Uses the exact machine compiled to WebAssembly.
use riscv_core::{BlockDevice, Entropy, Machine, MachineConfig};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
struct Disk(File);
impl BlockDevice for Disk {
    fn read_at(&mut self, o: u64, b: &mut [u8]) -> Result<(), String> {
        self.0
            .seek(SeekFrom::Start(o))
            .and_then(|_| self.0.read_exact(b))
            .map_err(|e| e.to_string())
    }
    fn write_at(&mut self, o: u64, b: &[u8]) -> Result<(), String> {
        self.0
            .seek(SeekFrom::Start(o))
            .and_then(|_| self.0.write_all(b))
            .map_err(|e| e.to_string())
    }
    fn flush(&mut self) -> Result<(), String> {
        self.0.sync_all().map_err(|e| e.to_string())
    }
}
struct Random(File);
impl Entropy for Random {
    fn fill(&mut self, b: &mut [u8]) -> Result<(), String> {
        self.0.read_exact(b).map_err(|e| e.to_string())
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 4 {
        eprintln!("usage: wasmnux-native ASSET_DIR DISK [--seconds N] [--input TEXT]");
        std::process::exit(2)
    }
    let dir = std::path::Path::new(&a[1]);
    let read = |n| std::fs::read(dir.join(n));
    let disk = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&a[2])?;
    let sectors = disk.metadata()?.len() / 512;
    let mut seconds = 30;
    let mut input = String::new();
    let mut no_initrd = false;
    let mut cmdline = "console=ttyS0 earlycon=sbi rdinit=/init".to_string();
    let mut i = 3;
    while i < a.len() {
        match a[i].as_str() {
            "--seconds" => {
                i += 1;
                seconds = a.get(i).ok_or("missing seconds")?.parse()?;
            }
            "--no-initrd" => {
                no_initrd = true;
            }
            "--cmdline" => {
                i += 1;
                cmdline = a.get(i).ok_or("missing cmdline")?.clone();
            }
            "--input" => {
                i += 1;
                input = a.get(i).ok_or("missing input")?.clone();
            }
            x => return Err(format!("unknown option {x}").into()),
        }
        i += 1;
    }
    let initrd_bytes = if no_initrd {
        Vec::new()
    } else {
        read("initramfs.cpio")?
    };
    let cfg = MachineConfig {
        ram_bytes: 256 * 1024 * 1024,
        disk_sectors: sectors,
        cmdline,
    };
    let mut m = Machine::new(
        &cfg,
        &read("fw_jump.bin")?,
        &read("Image")?,
        &initrd_bytes,
        Arc::new(Mutex::new(Disk(disk))),
        Arc::new(Mutex::new(Random(File::open("/dev/urandom")?))),
    )?;
    let start = Instant::now();
    let mut sent = false;
    let mut last = Instant::now();
    while !m.powered_off && start.elapsed() < Duration::from_secs(seconds) {
        if !sent && start.elapsed() > Duration::from_secs(seconds / 2) {
            m.console.input(input.as_bytes())?;
            sent = true;
        }
        m.step(200_000)?;
        if last.elapsed() > Duration::from_millis(100) {
            let out = m.console.drain();
            std::io::stdout().write_all(&out)?;
            std::io::stdout().flush()?;
            last = Instant::now();
        }
    }
    std::io::stdout().write_all(&m.console.drain())?;
    eprintln!(
        "\nretired={} pc={:#x} mode={:?} powered_off={}",
        m.cpu.instret,
        m.cpu.pc,
        m.cpu.mode(),
        m.powered_off
    );
    Ok(())
}
