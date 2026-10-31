#!/usr/bin/env python3
"""Fetch a pinned historical, volatile Linux diagnostic guest (not a profile)."""

from __future__ import annotations

import hashlib
import os
from pathlib import Path
import tarfile
import tempfile
from typing import BinaryIO
from urllib.request import urlopen

COMMIT = "8ee69d7a5dc7ef6d8b2bda96bf86d2923f2cf176"
PREFIX = f"riscv-rust-{COMMIT}/"
URL = f"https://codeload.github.com/takahirox/riscv-rust/tar.gz/{COMMIT}"
# Exact upstream source paths, destination names, sizes and SHA-256 digests.
ASSETS = {
    "wasm/web/riscv_emu_rust_wasm.js": (
        "riscv_emu_rust_wasm.js", 15694,
        "b51a13f2be49a8286e96e5c6d51dd677950fbdf4675b7fa80ff356da76878d05",
    ),
    "wasm/web/riscv_emu_rust_wasm_bg.wasm": (
        "riscv_emu_rust_wasm_bg.wasm", 158147,
        "e1804062b8a40bfe27af9a393bee291b0e8228435ebbc47a19d45a1faa8e3f65",
    ),
    "resources/linux/opensbi/fw_payload.elf": (
        "fw_payload.elf", 9907888,
        "3ce5529fe302144732b22636d614965bea3ac09999cfdb42912e74c33c6396e6",
    ),
    "resources/linux/rootfs.img": (
        "rootfs.img", 52428800,
        "273067980c4d8ad3898d505c1786ef69ecf6b33b8373942089a8b0ffe84b3409",
    ),
}


def extract_assets(stream: BinaryIO, dest: Path, assets=ASSETS) -> None:
    """Only write exact predeclared regular files; verify everything before installation."""
    dest.mkdir(parents=True, exist_ok=True)
    seen: set[str] = set()
    with tempfile.TemporaryDirectory(prefix="probe-", dir=dest) as tmp:
        stage = Path(tmp)
        with tarfile.open(fileobj=stream, mode="r|gz") as archive:
            for member in archive:
                if not member.name.startswith(PREFIX):
                    continue
                source = member.name[len(PREFIX):]
                if source not in assets:
                    continue
                output, expected_size, expected_hash = assets[source]
                if source in seen or not member.isfile() or member.size != expected_size:
                    raise ValueError(f"Invalid or duplicate archive member: {source}")
                if Path(output).name != output:
                    raise ValueError("Invalid configured destination")
                reader = archive.extractfile(member)
                if reader is None:
                    raise ValueError(f"Cannot read archive member: {source}")
                digest = hashlib.sha256()
                with reader, (stage / output).open("wb") as target:
                    while chunk := reader.read(1024 * 1024):
                        digest.update(chunk)
                        target.write(chunk)
                if digest.hexdigest() != expected_hash:
                    raise ValueError(f"SHA-256 mismatch: {source}")
                seen.add(source)
        if seen != set(assets):
            raise ValueError(f"Missing archive members: {sorted(set(assets) - seen)}")
        for output, _, _ in assets.values():
            os.replace(stage / output, dest / output)


def main() -> None:
    dest = Path(__file__).resolve().parent.parent / "public" / "guest"
    print(f"Downloading historical diagnostic artifacts from {URL}", flush=True)
    print("This guest is obsolete, offline, and NON-PERSISTENT; it is not a supported Linux profile.", flush=True)
    with urlopen(URL, timeout=60) as response:
        extract_assets(response, dest)
    print(f"Installed verified diagnostic artifacts in {dest}")


if __name__ == "__main__":
    main()
