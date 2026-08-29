// Supplementary developer-API popup opening. This is NOT an actual toolbar click.
// Uses only the dedicated agent-browser connection and the unpacked Boopmark worker.
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
const launcher = fileURLToPath(new URL('./chrome-browser.mjs', import.meta.url));
const command = spawnSync(process.execPath, [launcher, 'get', 'cdp-url'], { encoding: 'utf8' });
if (command.status !== 0) throw new Error('The dedicated agent-browser session is unavailable.');
const endpoint = command.stdout.split('\n').find(line => line.startsWith('ws://127.0.0.1:'));
if (!endpoint) throw new Error('Expected a loopback developer connection for this test browser.');
const origin = new URL(endpoint).origin.replace('ws:', 'http:');
const targets = await (await fetch(`${origin}/json/list`)).json();
const workers = targets.filter(target => target.type === 'service_worker' && /^chrome-extension:\/\/[a-p]{32}\/worker\.js$/.test(target.url));
if (workers.length !== 1) throw new Error('Reload the sole unpacked Boopmark extension, return to the test article, then retry while its worker is active.');
const ws = new WebSocket(workers[0].webSocketDebuggerUrl);
await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; });
const response = new Promise((resolve, reject) => {
  const timeout = setTimeout(() => reject(new Error('Programmatic popup did not open within 10 seconds.')), 10_000);
  ws.onmessage = ({ data }) => {
    const message = JSON.parse(data);
    if (message.id !== 1) return;
    clearTimeout(timeout);
    if (message.error || message.result?.exceptionDetails) {
      const reason = message.error?.message || message.result?.exceptionDetails?.exception?.description || message.result?.exceptionDetails?.text;
      reject(new Error(`Chrome rejected programmatic popup opening: ${String(reason).slice(0, 500)}`));
    }
    else resolve(message.result);
  };
});
ws.send(JSON.stringify({ id: 1, method: 'Runtime.evaluate', params: {
  expression: 'chrome.windows.getLastFocused().then(async window => { await chrome.windows.update(window.id, {focused: true}); return chrome.action.openPopup({windowId: window.id}); })',
  awaitPromise: true, returnByValue: true,
} }));
try { await response; console.log('Opened actual popup via developer API (supplementary only; NOT toolbar coverage).'); }
finally { ws.close(); }
