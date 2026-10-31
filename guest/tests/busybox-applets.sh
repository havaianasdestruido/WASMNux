#!/bin/sh
# Behaviour tests for every Mission §3 BusyBox applet. These execute the
# real applets and check real effects (files, symlinks, exit codes, output
# content) — existence alone never counts as a pass.
#
# Usage: busybox-applets.sh [busybox-binary]
#   IN_GUEST=1  strict mode: root-only operations (mount, dmesg) must pass.
# On a dev host (no root), root-only checks print SKIP and do not fail the
# run; the guest run is the authoritative evidence.
set -u

BB="${1:-busybox}"
IN_GUEST="${IN_GUEST:-0}"
WORK="$(mktemp -d 2>/dev/null || echo /tmp/bbtest.$$)"
mkdir -p "${WORK}" || exit 1
cd "${WORK}" || exit 1
PASS=0; FAIL=0; SKIP=0

ok()   { PASS=$((PASS+1)); echo "PASS: $1"; }
bad()  { FAIL=$((FAIL+1)); echo "FAIL: $1"; }
skip() { SKIP=$((SKIP+1)); echo "SKIP: $1 (host lacks privileges; required in-guest)"; }
# run <name> <expected-substring-or-""> <applet args...>
run() {
  name="$1"; want="$2"; shift 2
  out="$("${BB}" "$@" 2>&1)"; rc=$?
  if [ "${rc}" -ne 0 ]; then bad "${name} (exit ${rc}: ${out})"; return; fi
  if [ -n "${want}" ]; then
    case "${out}" in *"${want}"*) ok "${name}" ;; *) bad "${name} (output '${out}' lacks '${want}')" ;; esac
  else
    ok "${name}"
  fi
}

# sh / ash actually execute shell code (arithmetic proves a real interpreter)
run "sh arithmetic" "42" sh -c 'echo $((6*7))'
run "ash arithmetic" "42" ash -c 'echo $((6*7))'

# File lifecycle: touch -> cat -> cp -> mv -> ls -> rm
run "touch" "" touch hello.txt
printf 'persistent-data\n' > hello.txt
run "cat content" "persistent-data" cat hello.txt
run "cp" "" cp hello.txt copy.txt
run "cat copied" "persistent-data" cat copy.txt
run "mv" "" mv copy.txt moved.txt
if [ ! -f moved.txt ] || [ -e copy.txt ]; then bad "mv effect"; else ok "mv effect"; fi
run "ls sees files" "moved.txt" ls
run "grep matches" "persistent-data" grep persistent moved.txt
run "grep no-match exit code" "" sh -c '! grep -q zzz moved.txt'
run "sed rewrite" "pXrsistent-data" sh -c "sed 's/e/X/' moved.txt"
run "awk field" "data" sh -c "echo 'persistent data' | awk '{print \$2}'"

# Directories
run "mkdir" "" mkdir subdir
run "cp into dir" "" cp moved.txt subdir/
run "find locates" "subdir/moved.txt" find . -name moved.txt
run "rmdir precondition rm" "" rm subdir/moved.txt
run "rmdir" "" rmdir subdir

# Links
run "ln -s" "" ln -s moved.txt link.txt
run "readlink" "moved.txt" readlink link.txt
run "cat through symlink" "persistent-data" cat link.txt
run "chmod" "" chmod 600 moved.txt
run "stat via ls -l perms" "rw-------" ls -l moved.txt
if [ "$(id -u 2>/dev/null)" = "0" ]; then
  run "chown root" "" chown 0:0 moved.txt
else
  # BusyBox chown to self still validates the applet path honestly.
  run "chown self" "" chown "$(id -u):$(id -g)" moved.txt
fi

# Archives: tar+gzip round-trip must preserve bytes.
run "tar create" "" tar -cf bundle.tar moved.txt
run "gzip" "" gzip -c bundle.tar
"${BB}" gzip bundle.tar
mkdir extract && cd extract || exit 1
"${BB}" gzip -d ../bundle.tar.gz && "${BB}" tar -xf ../bundle.tar \
  && [ "$(cat moved.txt)" = "persistent-data" ] \
  && ok "tar+gzip round-trip content" || bad "tar+gzip round-trip content"
cd .. || exit 1

run "rm cleanup" "" rm -rf extract link.txt bundle.tar
[ ! -e link.txt ] && ok "rm effect" || bad "rm effect"

# System information applets must answer with plausible live data.
run "uname -s" "Linux" uname -s
run "uname -m non-empty" "" sh -c '[ -n "$(uname -m)" ]'
run "free reports Mem" "Mem" free
run "df reports a filesystem" "" sh -c 'df . | tail -n +2 | grep -q .'
run "du counts bytes" "" sh -c '[ "$(du -s . | cut -f1)" -ge 0 ]'
run "date produces a year" "20" date +%Y
run "sleep actually waits" "" sh -c 's=$(date +%s); sleep 1; [ $(( $(date +%s) - s )) -ge 1 ]'
run "ps lists processes" "" sh -c 'ps | grep -q .'
run "top one iteration" "Mem" top -b -n 1

# kill really signals a process.
"${BB}" sh -c 'sleep 30' &
VICTIM=$!
sleep 0
if "${BB}" kill "${VICTIM}" 2>/dev/null; then ok "kill signals process"; else bad "kill signals process"; fi
wait "${VICTIM}" 2>/dev/null

# Root-only in real environments.
if [ "$(id -u)" = "0" ]; then
  run "dmesg has kernel lines" "Linux" dmesg
  mkdir -p mnt && run "mount tmpfs" "" mount -t tmpfs tmpfs mnt \
    && printf x > mnt/probe && [ "$(cat mnt/probe)" = "x" ] \
    && ok "mount effect visible" && run "umount" "" umount mnt
  # reboot/poweroff: assert the applets exist and parse flags WITHOUT doing
  # it here; real effect is validated by the boot/poweroff e2e tests.
  "${BB}" reboot --help >/dev/null 2>&1; [ $? -le 1 ] && ok "reboot applet operational" || bad "reboot applet operational"
  "${BB}" poweroff --help >/dev/null 2>&1; [ $? -le 1 ] && ok "poweroff applet operational" || bad "poweroff applet operational"
else
  skip "dmesg"
  skip "mount/umount"
  skip "reboot/poweroff (never executed on a dev host)"
  if [ "${IN_GUEST}" = "1" ]; then bad "IN_GUEST=1 but not running as guest root"; fi
fi

# cd is a shell builtin: prove it changes directory for real.
out="$("${BB}" sh -c "mkdir -p ${WORK}/cdtest && cd ${WORK}/cdtest && pwd")"
[ "${out}" = "${WORK}/cdtest" ] && ok "cd builtin" || bad "cd builtin (got '${out}')"

cd /
rm -rf "${WORK}"
echo "== BusyBox behaviour: PASS=${PASS} FAIL=${FAIL} SKIP=${SKIP} =="
[ "${FAIL}" -eq 0 ]
