import { resolve } from 'node:path';
import { assertVersionsMatch, readReleaseVersions } from './version-lib.mjs';

const repository = resolve(import.meta.dirname, '../..');
const expected = process.argv[2] ?? JSON.parse(await (await import('node:fs/promises')).readFile(resolve(repository, 'package.json'), 'utf8')).version;
const versions = await readReleaseVersions(repository);
assertVersionsMatch(versions, expected);
process.stdout.write(`All release version sources match ${expected}\n`);
