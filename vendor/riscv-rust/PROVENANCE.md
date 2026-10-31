# Diagnostic emulator source

`src/` and `LICENSE` originate from
[takahirox/riscv-rust](https://github.com/takahirox/riscv-rust) commit
`8ee69d7a5dc7ef6d8b2bda96bf86d2923f2cf176` (MIT). The standalone Cargo
manifest uses a locally vendored `fnv` dependency. The source is reformatted
with rustfmt; WASMNux also replaces deprecated constant spellings and adds the
`FCVT.L.D` and `FCVT.D.LU` instructions and their tests in `src/cpu.rs`. See
the project's test report for behavior and limitations. This historical
emulator is a diagnostic, not a validated general-purpose RISC-V implementation.

The `fnv` source at `../fnv` is version 1.0.7, Git commit
`4b4784ebfd3332dc61f0640764d6f1140e03a9ab`, reformatted with rustfmt and
with its MIT/Apache-2.0 licenses.
