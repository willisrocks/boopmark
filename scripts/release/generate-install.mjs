import { writeFile } from 'node:fs/promises';
import { renderInstallGuide } from './version-lib.mjs';

const [version, output = 'INSTALL.md', repository = 'willisrocks/boopmark'] = process.argv.slice(2);
if (!version) throw new Error('usage: node scripts/release/generate-install.mjs VERSION [OUTPUT] [OWNER/REPO]');
await writeFile(output, renderInstallGuide(version, repository));
process.stdout.write(`${output}\n`);
