#!/usr/bin/env python3
"""Emit the files `make syncconfig` would produce, without the C kconfig tools.

The kernel's own configurator (`scripts/kconfig/conf`) is built with flex and
bison. A normal distribution has both, but a bare container often does not, and
without them the kernel cannot even resolve `.config` -- the build stops at the
first `make` invocation. This script reads the same Kconfig sources with
`kconfiglib` (pure Python) and writes the same generated files.

Usage:
  gen-kconfig.py <srctree> <fragment.config> <objtree>

Semantics match `merge_config.sh -m` followed by `olddefconfig`: start from the
Kconfig defaults (which already select the architecture via ARCH), then overlay
only the symbols named in the fragment.

Linux 7.x uses bits of Kconfig syntax that stock kconfiglib does not know. Both
adaptations are narrow and are the only places this script diverges from the
upstream tool:

* `transitional` and bare `modules` are marker lines the C parser accepts as
  no-ops (`config_option: T_TRANSITIONAL T_EOL` in scripts/kconfig/parser.y).
  kconfiglib treats them as unknown syntax, so they are stripped from the
  build tree's Kconfig files before parsing.
* `depends on X if Y` means `X || (Y == n)` -- see `menu_add_dep()` in
  scripts/kconfig/menu.c, which writes exactly "Y != n --> X; Y == n --> y".
  The rewrite is textual, applied to the same build-tree files.

Both edits touch only Kconfig sources in the throwaway build tree; nothing that
affects compiled output. They are idempotent: a marker comment records that a
file has been processed.
"""

import os
import sys
import time

import kconfiglib

MARKER = "# wasmnux-kconfig-preprocessed"

# CONFIG_DTC only gates the *host* dtc/fdtoverlay binaries. This build produces
# no DTBs and never runs dt_binding_check, and dtc itself needs flex, so leaving
# it on would reintroduce the dependency this script exists to avoid. No C
# source reads CONFIG_DTC.
SKIP = {"DTC"}


def preprocess_kconfig_tree(srctree):
    """Remove Linux 7.x syntax that stock kconfiglib rejects."""
    import re

    marker_line = re.compile(r"^[ \t]*transitional[ \t]*$")
    bare_modules = re.compile(r"^[ \t]*modules[ \t]*$")
    # Matched against the code half of a line only (see below), so a trailing
    # comment can never be mistaken for the `if <cond>` suffix.
    depends_if = re.compile(
        r"^([ \t]*)depends[ \t]+on[ \t]+(.*?)[ \t]+if[ \t]+(.+?)[ \t]*$"
    )

    touched = 0
    originals = {}
    for dirpath, dirnames, filenames in os.walk(srctree):
        # Skip build output and the vcs database, which never hold Kconfig input.
        dirnames[:] = [d for d in dirnames if d not in (".git", "output", "include")]
        for name in filenames:
            if not (name.startswith("Kconfig") or name.startswith("Kconfig.")):
                continue
            path = os.path.join(dirpath, name)
            try:
                with open(path, encoding="utf-8", errors="surrogateescape",
                          newline="") as fh:
                    # newline="" keeps line endings intact, but readlines()
                    # would also split on a lone \r. Kconfig files in the
                    # kernel tree are LF-terminated; split on \n only so a
                    # stray \r can never merge two entries into one line.
                    lines = [ln + "\n" for ln in fh.read().split("\n")]
                    lines[-1] = lines[-1][:-1]
                    if lines[-1] == "":
                        lines.pop()
            except OSError:
                continue
            if any(MARKER in ln for ln in lines[:5]):
                continue  # already processed

            out, changed = [], False
            for ln in lines:
                if marker_line.match(ln) or bare_modules.match(ln):
                    changed = True
                    continue
                # Split off a trailing comment before matching. A comment
                # that merely mentions "if" (e.g. `# if ACPI_NFIT=m, ...`)
                # must not be rewritten as syntax.
                code, sep, comment = ln.partition("#")
                newline = ""
                if code.endswith("\n"):
                    code, newline = code[:-1], "\n"
                m = depends_if.match(code)
                if m:
                    indent, dep, cond = m.groups()
                    if sep and not comment.startswith(" "):
                        sep = " " + sep
                    out.append("%sdepends on %s || %s = n%s%s%s"
                               % (indent, dep, cond, sep, comment, newline))
                    changed = True
                    continue
                out.append(ln)

            if changed:
                originals[path] = "".join(lines)
                out.insert(0, MARKER + "\n")
                with open(path, "w", encoding="utf-8", errors="surrogateescape") as fh:
                    fh.writelines(out)
                touched += 1

    return touched, originals


def restore_kconfig_tree(originals):
    """Undo preprocess_kconfig_tree().

    The kernel derives its release string from `git describe`, and
    CONFIG_LOCALVERSION_AUTO=y appends "-dirty" when the work tree has
    modifications. Leaving the rewritten Kconfig files in place therefore
    produced a kernel reporting 7.3.1-dirty, which breaks `uname -r` checks in
    the guest. Restoring the exact original bytes after parsing keeps the tree
    pristine.
    """
    for path, text in originals.items():
        with open(path, "w", encoding="utf-8", errors="surrogateescape") as fh:
            fh.write(text)


def patch_int_comparison():
    """Treat an unset int/hex symbol as 0, matching the C implementation.

    kconfiglib converts a symbol with `int(sym.str_value, base)` and lets an
    empty string raise ValueError, after which it falls back to comparing the
    text. The C tool always has a numeric value, so `range 1 10 if
    ARCH_FORCE_MAX_ORDER = 0` takes the first branch on architectures that never
    define ARCH_FORCE_MAX_ORDER (RISC-V among them). Getting this wrong emitted
    CONFIG_PAGE_BLOCK_MAX_ORDER=10 while MAX_PAGE_ORDER stayed at its header
    default, and the kernel refused to compile.
    """
    original = kconfiglib._sym_to_num

    def _sym_to_num(sym):
        if sym.orig_type is kconfiglib.INT and not sym.str_value:
            return 0
        if sym.orig_type is kconfiglib.HEX and not sym.str_value:
            return 0
        return original(sym)

    kconfiglib._sym_to_num = _sym_to_num


def main():
    srctree, fragment, objtree = sys.argv[1:4]

    stripped, originals = preprocess_kconfig_tree(srctree)
    if stripped:
        print("[gen-kconfig] normalised Linux 7.x syntax in %d Kconfig files" % stripped)

    patch_int_comparison()

    os.environ.update(
        srctree=srctree,
        ARCH="riscv",
        SRCARCH="riscv",
        CC="clang",
        LD="ld.lld",
        CLANG_FLAGS="-fintegrated-as",
    )

    # Symbols the fragment names explicitly. The allnoconfig sweep must never
    # touch these: a fragment entry whose dependency is only satisfied later
    # (VIRTIO_NET depends on VIRTIO) would otherwise be reset between passes.
    pinned = set()
    with open(fragment) as fh:
        for line in fh:
            line = line.strip()
            if line.startswith("CONFIG_") and "=" in line:
                pinned.add(line[len("CONFIG_"):].partition("=")[0])
            elif line.startswith("# CONFIG_") and line.endswith(" is not set"):
                pinned.add(line.split()[1][len("CONFIG_"):])

    try:
        kconf = kconfiglib.Kconfig(os.path.join(srctree, "Kconfig"),
                                   warn_to_stderr=False)

        def allnoconfig():
            """Apply `make allnoconfig` semantics.

            Every visible bool/tristate becomes n unless it is allnoconfig_y or
            its first default evaluates to y *and* that default's condition
            holds. Symbols named by the fragment are left alone.
            """
            for sym in kconf.unique_defined_syms:
                if sym.orig_type not in (kconfiglib.BOOL, kconfiglib.TRISTATE):
                    continue
                if sym.name in pinned:
                    continue
                if sym.is_allnoconfig_y:
                    sym.set_value(2)
                    continue
                for node in sym.nodes:
                    if not node.prompt:
                        continue
                    if node.defaults:
                        # A default is (value, condition); both must hold, or
                        # `default y if PROFILING` wins even while PROFILING=n.
                        value, cond = node.defaults[0]
                        effective = min(kconfiglib.expr_value(value),
                                        kconfiglib.expr_value(cond))
                        sym.set_value(effective if effective == 2 else 0)
                    else:
                        sym.set_value(0)
                    break

        # Pass 1: baseline, as if `make allnoconfig` had run. Loading the
        # fragment straight onto kconfiglib's *defaults* is not equivalent: a
        # symbol defaulting to y that only becomes visible because of a fragment
        # entry keeps that y. CONFIG_EXPERT=y makes EXPERT select DEBUG_KERNEL,
        # which dragged FRAME_POINTER in and produced a measurably slower
        # kernel than the config describes.
        allnoconfig()

        # Pass 2: overlay the fragment, settle dependencies (so symbols the
        # fragment enables become visible), then re-normalise anything that
        # *newly* appeared, and re-apply the fragment last so it always wins.
        kconf.load_config(fragment, replace=False)
        allnoconfig()
        kconf.load_config(fragment, replace=False)
    finally:
        # Restore before anything can read `git describe` again.
        restore_kconfig_tree(originals)

    dotconfig = os.path.join(objtree, ".config")
    kconf.write_config(dotconfig, header="# WASMNux resolved configuration\n")

    auto, hdr, rustcfg = [], [], []
    for sym in sorted(kconf.unique_defined_syms, key=lambda s: s.name):
        name, val = sym.name, sym.str_value
        if name in SKIP:
            continue

        if sym.type in (kconfiglib.BOOL, kconfiglib.TRISTATE):
            if val not in ("y", "m"):
                continue
            auto.append("CONFIG_%s=%s" % (name, val))
            hdr.append("#define CONFIG_%s 1" % name)
            rustcfg.append('--cfg "%s"' % name)
            if val == "m":
                hdr.append("#define CONFIG_%s_MODULE 1" % name)
                rustcfg.append('--cfg "%s_MODULE"' % name)

        elif sym.type in (kconfiglib.INT, kconfiglib.HEX):
            if not val:
                # A symbol behind a disabled dependency still needs a concrete
                # value when headers compare against it in the preprocessor:
                # include/linux/cpumask.h does `NR_CPUS == 1`, and with SMP=n
                # CONFIG_NR_CPUS must be 1 (include/linux/threads.h).
                if sym.visibility and sym.defaults:
                    val = sym.defaults[0][0].str_value
                elif name == "NR_CPUS":
                    val = "1"
            if not val:
                # Genuinely undefined: headers test these with #ifdef, e.g.
                # include/linux/mmzone.h for ARCH_FORCE_MAX_ORDER.
                continue
            auto.append("CONFIG_%s=%s" % (name, val))
            hdr.append("#define CONFIG_%s %s" % (name, val))
            rustcfg.append('--cfg "%s=%s"' % (name, val))

        else:  # STRING
            # No quoting in auto.conf: values such as
            # "-mllvm -max-store-memset=4294967294" must word-split into
            # separate arguments, or clang receives one bogus argument.
            # Empty strings are still emitted, because
            # arch/riscv/kernel/vdso/note.S expands CONFIG_BUILD_SALT inside
            # .asciz and fails to assemble if it is missing entirely.
            auto.append("CONFIG_%s=%s" % (name, val))
            hdr.append('#define CONFIG_%s "%s"' % (name, val))
            rustcfg.append('--cfg "%s=\\"%s\\""' % (name, val))

    for rel, body, header in (
        ("include/config/auto.conf", auto, None),
        ("include/generated/autoconf.h", hdr,
         "/* WASMNux generated configuration */"),
        ("include/generated/rustc_cfg", rustcfg, None),
    ):
        path = os.path.join(objtree, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as fh:
            if header:
                fh.write(header + "\n")
            fh.write("\n".join(body) + "\n")

    cmd = os.path.join(objtree, "include/config/auto.conf.cmd")
    os.makedirs(os.path.dirname(cmd), exist_ok=True)
    with open(cmd, "w") as fh:
        fh.write("# generated by gen-kconfig.py; no syncconfig dependencies\n")

    # .config must be older than the generated files, otherwise make reruns
    # syncconfig -- which is exactly the flex-dependent target being avoided.
    now = time.time()
    os.utime(dotconfig, (now - 7200, now - 7200))
    for rel in ("include/config/auto.conf", "include/config/auto.conf.cmd",
                "include/generated/autoconf.h", "include/generated/rustc_cfg"):
        os.utime(os.path.join(objtree, rel), (now, now))

    enabled = sum(1 for s in kconf.unique_defined_syms if s.str_value in ("y", "m"))
    print("[gen-kconfig] %d enabled symbols -> %d auto.conf entries" %
          (enabled, len(auto)))


if __name__ == "__main__":
    main()
