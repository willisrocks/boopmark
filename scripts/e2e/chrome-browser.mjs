// Start the primary headed agent-browser harness with the unpacked build.
// A dedicated profile is intentional: never use an everyday browser profile.
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, readdirSync } from 'node:fs';
import { homedir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const extension = path.join(root, 'extensions/chrome');
const cli = path.join(root, 'node_modules/.bin/agent-browser');
const profile = path.resolve(process.env.CHROME_EXTENSION_PROFILE || path.join(root, '.cache/boopmark-extension/local-profile'));
const args = process.argv.slice(2);
const relativeToTestOutput = path.relative(path.join(root, 'test-results'), profile);
if (args[0] !== 'close' && (relativeToTestOutput === '' || (!relativeToTestOutput.startsWith(`..${path.sep}`) && relativeToTestOutput !== '..' && !path.isAbsolute(relativeToTestOutput)))) {
  throw new Error('Browser profiles must be outside test-results: Playwright clears that directory. Use .cache/boopmark-extension/ instead.');
}
const session = process.env.CHROME_EXTENSION_SESSION || `boopmark-extension-${createHash('sha256').update(root).digest('hex').slice(0, 8)}`;
const headed = process.env.CHROME_EXTENSION_HEADED !== 'false';
let executable = process.env.AGENT_BROWSER_EXECUTABLE_PATH;
if (!executable && process.platform === 'darwin') {
  const installed = path.join(homedir(), '.agent-browser/browsers');
  if (existsSync(installed)) {
    executable = readdirSync(installed).sort((a, b) => b.localeCompare(a, undefined, { numeric: true }))
      .map(name => path.join(installed, name, 'Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing'))
      .find(candidate => existsSync(candidate));
  }
}
if (!existsSync(cli)) throw new Error('Run npm ci first.');
if (!existsSync(path.join(extension, 'manifest.json'))) throw new Error('Unpacked extension is missing.');
if (!executable) throw new Error('Run npx agent-browser install; set AGENT_BROWSER_EXECUTABLE_PATH to an extension-capable Chromium binary.');
if (args[0] !== 'close') mkdirSync(profile, { recursive: true, mode: 0o700 });
const manifest = JSON.parse(readFileSync(path.join(extension, 'manifest.json'), 'utf8'));
const buildHash = createHash('sha256');
function hashFiles(directory) {
  for (const entry of readdirSync(directory, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
    if (entry.name === 'tests' || entry.name === 'README.md') continue;
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) hashFiles(file);
    else { buildHash.update(path.relative(extension, file)); buildHash.update(readFileSync(file)); }
  }
}
hashFiles(extension);
const command = args.length ? args : ['open', 'http://127.0.0.1:4011/article'];
console.log(`Boopmark ${manifest.version}; on-disk build ${buildHash.digest('hex').slice(0, 16)}; ${headed ? 'headed' : 'HEADLESS (supplementary only)'}; agent-browser session ${session}; profile ${profile}`);
const result = spawnSync(cli, [
  '--session', session, '--headed', String(headed), '--profile', profile,
  '--executable-path', executable, '--extension', extension, ...command,
], { cwd: root, stdio: 'inherit' });
process.exit(result.status ?? 1);
