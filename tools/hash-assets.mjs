import process from 'node:process';
import { readdir, readFile, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
const dir = process.argv[2] ?? 'web/public/assets';
const manifest = {};
for (const name of await readdir(dir)) {
  if (name === 'manifest.json') continue;
  manifest[name] = createHash('sha256')
    .update(await readFile(`${dir}/${name}`))
    .digest('hex');
}
await writeFile(`${dir}/manifest.json`, JSON.stringify(manifest, null, 2) + '\n');
