// Safely invoke the native outside-click helper only for this checkout's exact
// isolated local fixture session, profile, process, and non-production page.
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { fixtureSessionAllowed } from './chrome-popup-control.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const launcher = path.join(root, 'scripts/e2e/chrome-browser.mjs');
const nativeHelper = path.join(root, 'scripts/e2e/chrome-dismiss-popup.applescript');

function fail(message) {
  process.stderr.write(`${message}\n`);
  process.exit(1);
}

const [pidText] = process.argv.slice(2);
if (!pidText || !/^[1-9][0-9]*$/.test(pidText) || process.argv.length !== 3) {
  fail('Usage: node scripts/e2e/chrome-dismiss-popup.mjs <exact-browser-pid>');
}
if (!fixtureSessionAllowed()) fail('Outside-click control requires the exact isolated local QA session and profile.');

const expectedProfile = path.resolve(process.env.CHROME_EXTENSION_PROFILE);
const expectedExtension = path.join(root, 'extensions/chrome');
const processResult = spawnSync('ps', ['-p', pidText, '-o', 'command='], {
  cwd: root, encoding: 'utf8', timeout: 10_000,
});
const command = processResult.status === 0 ? processResult.stdout.trim() : '';
const exactFlag = (name, value) => {
  const token = `${name}=${value}`.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return new RegExp(`(?:^|\\s)${token}(?=\\s|$)`).test(command);
};
if (!command.includes('Google Chrome for Testing')
    || !exactFlag('--user-data-dir', expectedProfile)
    || !exactFlag('--load-extension', expectedExtension)
    || !exactFlag('--disable-extensions-except', expectedExtension)) {
  fail('The requested PID is not this checkout’s isolated extension browser.');
}

const urlResult = spawnSync(process.execPath, [launcher, 'get', 'url'], {
  cwd: root, encoding: 'utf8', timeout: 10_000, env: process.env,
});
const currentURL = urlResult.status === 0
  ? urlResult.stdout.split(/\r?\n/).map(line => line.trim()).find(line => line.startsWith('http://127.0.0.1:4011/article'))
  : null;
if (!currentURL) fail('The dedicated session is not showing the static local fixture article.');
try {
  const parsed = new URL(currentURL);
  if (parsed.origin !== 'http://127.0.0.1:4011' || !parsed.pathname.startsWith('/article')) throw new Error('fixture');
} catch {
  fail('The dedicated session is not showing the static local fixture article.');
}

const result = spawnSync('osascript', [nativeHelper, pidText], {
  cwd: root, stdio: 'inherit', timeout: 10_000,
});
process.exit(result.status ?? 1);
