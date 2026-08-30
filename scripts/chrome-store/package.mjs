import { copyFile, cp, mkdir, mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, dirname, join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const repository = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const source = join(repository, 'extensions/chrome');
const manifest = JSON.parse(await readFile(join(source, 'manifest.json'), 'utf8'));
const outputDirectory = join(repository, 'dist');
const output = join(outputDirectory, `boopmark-chrome-${manifest.version}.zip`);
const staging = await mkdtemp(join(tmpdir(), 'boopmark-chrome-store-'));
const files = [
  'manifest.json',
  'api.js',
  'core.js',
  'worker.js',
  'popup.html',
  'popup.js',
  'styles.css',
  'logo.svg',
];

try {
  await mkdir(outputDirectory, { recursive: true });
  for (const file of files) await copyFile(join(source, file), join(staging, file));
  await cp(join(source, 'icons'), join(staging, 'icons'), { recursive: true });
  await rm(output, { force: true });

  const zipped = spawnSync(
    'zip',
    ['-X', '-q', '-r', output, ...files, 'icons'],
    { cwd: staging, encoding: 'utf8' },
  );
  if (zipped.status !== 0) throw new Error(zipped.stderr || 'zip failed');
  process.stdout.write(`${basename(output)}\n${output}\n`);
} finally {
  await rm(staging, { recursive: true, force: true });
}
