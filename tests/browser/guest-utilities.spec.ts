import { expect, test } from "@playwright/test";

test("historical BusyBox guest performs real filesystem, process and utility operations", async ({
  page,
}) => {
  await page.goto("/tests/browser/worker-harness.html");
  await page.getByRole("button", { name: "Start" }).click();
  const output = page.locator("#output");
  await expect(output).toContainText("Please press Enter to activate this console", {
    timeout: 90_000,
  });
  await page.getByLabel("Serial input", { exact: true }).fill("\\n");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(output).toContainText("/ #", { timeout: 20_000 });
  const script = String.raw`cat > /tmp/wasmnux-test.sh <<'END_WASMNUX_TEST'
#!/bin/sh
set -ex
mkdir -p /tmp/wasmnux-check
cd /tmp/wasmnux-check
printf 'alpha\nbeta\n' > input
ls input > /dev/null
cp input copied
mv copied moved
grep -q '^alpha$' moved
sed 's/alpha/omega/' moved | grep -q '^omega$'
awk '/beta/{found=1} END{exit !found}' moved
find . -name moved | grep -q moved
ln -s moved link
test "$(readlink link)" = moved
touch other
chmod 700 other
test -x other
chown 123:456 other
ls -ln other | awk '{exit !($3 == 123 && $4 == 456)}'
tar -cf archive.tar input
mkdir extracted
tar -xf archive.tar -C extracted
cmp input extracted/input
gzip -c input > input.gz
gzip -dc input.gz | grep -q beta
ash -c 'test -f /tmp/wasmnux-check/input'
mount -t proc proc /proc 2>/dev/null || true
test -r /proc/meminfo
ps > /tmp/wasmnux-ps
free > /tmp/wasmnux-free
dmesg > /dev/null
uname -m | grep -q riscv64
df . > /dev/null
du -s . > /dev/null
date -u +%Y | grep -q '^[0-9][0-9][0-9][0-9]$'
sleep 60 &
p=$!
kill "$p"
wait "$p" 2>/dev/null || true
sleep 0
mkdir mountpoint
mount -t tmpfs tmpfs mountpoint
touch mountpoint/inside
test -f mountpoint/inside
umount mountpoint
rmdir mountpoint
top -b -n 1 > /tmp/wasmnux-top
grep -q 'Mem:' /tmp/wasmnux-top
rm -f link moved other archive.tar input.gz input extracted/input
rmdir extracted
cd /
rmdir /tmp/wasmnux-check
END_WASMNUX_TEST
sh /tmp/wasmnux-test.sh && printf 'CHECK_%s\n' PASS || printf 'CHECK_%s\n' FAIL
`;
  await page.getByLabel("Raw serial input").fill(script);
  await page.getByRole("button", { name: "Send raw" }).click();
  const status = page.locator("#status");
  await expect
    .poll(
      async () => {
        const guestStatus = (await status.textContent()) ?? "";
        if (guestStatus.startsWith("error:")) return guestStatus;
        const text = (await output.textContent()) ?? "";
        if (text.includes("CHECK_FAIL")) return "CHECK_FAIL";
        if (text.includes("CHECK_PASS")) return "CHECK_PASS";
        return "waiting";
      },
      { timeout: 75_000 },
    )
    .not.toBe("waiting");
  const guestStatus = (await status.textContent()) ?? "";
  expect(guestStatus, `Guest failed during utility verification: ${guestStatus}`).toBe("running");
  await expect(output).toContainText("CHECK_PASS");
  await expect(output).not.toContainText("CHECK_FAIL");
});
