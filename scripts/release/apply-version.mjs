import { readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { assertReleaseVersion, syncVersion } from './version-lib.mjs';

const [version, buildNumber] = process.argv.slice(2);
assertReleaseVersion(version);
if (!buildNumber) throw new Error('usage: node scripts/release/apply-version.mjs VERSION IOS_BUILD_NUMBER');

const repository = resolve(import.meta.dirname, '../..');
for (const relative of ['package.json', 'package-lock.json']) {
  const path = resolve(repository, relative);
  const value = JSON.parse(await readFile(path, 'utf8'));
  value.version = version;
  if (relative === 'package-lock.json') value.packages[''].version = version;
  await writeFile(path, `${JSON.stringify(value, null, 2)}\n`);
}

await syncVersion(repository, version, buildNumber);
process.stdout.write(`Applied release version ${version} (iOS build ${buildNumber})\n`);
