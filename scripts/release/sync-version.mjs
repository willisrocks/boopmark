import { resolve } from 'node:path';
import { syncVersion } from './version-lib.mjs';

const [version, buildNumber] = process.argv.slice(2);
if (!version || !buildNumber) {
  throw new Error('usage: node scripts/release/sync-version.mjs VERSION IOS_BUILD_NUMBER');
}

await syncVersion(resolve(import.meta.dirname, '../..'), version, buildNumber);
process.stdout.write(`Synchronized release version ${version} (iOS build ${buildNumber})\n`);
