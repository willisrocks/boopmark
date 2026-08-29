// Control the actual action popup opened by Chrome's toolbar. This is a
// supplementary helper for the headed agent-browser session; it never opens
// a popup tab or falls back to another target.
import { spawnSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const launcher = path.join(root, 'scripts/e2e/chrome-browser.mjs');
const fixtureProfile = path.join(root, '.cache/boopmark-extension/local-qa-profile');
const expectedExtensionId = 'ggfienpplnccomboiahcllfpakbopane';
const expectedPopupURL = `chrome-extension://${expectedExtensionId}/popup.html`;
const allowedFields = new Set(['url', 'title', 'description', 'tags']);
const allowedButtons = new Set(['autofill-button', 'save-button', 'cancel-button', 'close-button', 'ack-button']);
const fixtureButtons = new Set(['settings-button', 'back-button', 'disconnect-button']);
const captureStatusIds = new Set(['url-error', 'metadata-status', 'save-status']);
const accessibilityIds = new Set([...allowedFields, ...allowedButtons, ...captureStatusIds]);
const safeFieldIds = ['url', 'title', 'description', 'tags'];
const safeStatusIds = ['metadata-status', 'save-status', 'connection-status', 'fatal-error', 'url-error'];
const popupTimeoutMs = 10_000;
let nextMessageId = 1;

class PopupControlError extends Error {}

function fail(message) {
  throw new PopupControlError(message);
}

export function fixtureSessionAllowed(env = process.env) {
  const profile = env.CHROME_EXTENSION_PROFILE;
  return env.CHROME_EXTENSION_SESSION === 'boopmark-extension-local-fdcc'
    && typeof profile === 'string' && path.isAbsolute(profile)
    && path.resolve(profile) === fixtureProfile;
}

export function buttonAllowed(button, env = process.env) {
  return allowedButtons.has(button) || (fixtureButtons.has(button) && fixtureSessionAllowed(env));
}

function requireFixtureSession() {
  if (!fixtureSessionAllowed()) fail('Fixture controls require the exact isolated local QA session and profile.');
}

export function projectAccessibility(nodes, observed) {
  if (!Array.isArray(nodes) || nodes.length > 1000 || !Array.isArray(observed) || observed.length > 64) fail('Accessibility snapshot exceeded its capture-only bound.');
  const allowed = new Map(observed.filter(item => accessibilityIds.has(item.id)
    && Number.isInteger(item.backendDOMNodeId) && item.backendDOMNodeId > 0
    && (item.kind === 'element' || (item.kind === 'status-text' && captureStatusIds.has(item.id))))
    .map(item => [item.backendDOMNodeId, item]));
  const projected = [];
  for (const node of nodes) {
    const item = allowed.get(node.backendDOMNodeId);
    if (!item || node.ignored) continue;
    const entry = { id: item.id, kind: item.kind, role: typeof node.role?.value === 'string' ? node.role.value.slice(0, 80) : null,
      name: typeof node.name?.value === 'string' ? node.name.value.slice(0, 500) : '' };
    for (const property of Array.isArray(node.properties) ? node.properties : []) {
      const value = property.value?.value;
      if (['focused', 'atomic'].includes(property.name) && typeof value === 'boolean') entry[property.name] = value;
      if (property.name === 'live' && ['off', 'polite', 'assertive'].includes(value)) entry.live = value;
      if (property.name === 'relevant' && typeof value === 'string' && /^(?:additions|removals|text|all)(?: (?:additions|removals|text|all))*$/.test(value)) entry.relevant = value;
    }
    projected.push(entry);
    if (projected.length > 64) fail('Accessibility snapshot exceeded its capture-only bound.');
  }
  return projected;
}

function cdpEndpoint() {
  const result = spawnSync(process.execPath, [launcher, 'get', 'cdp-url'], {
    cwd: root,
    encoding: 'utf8',
    env: process.env,
    timeout: popupTimeoutMs,
  });
  if (result.error || result.status !== 0) fail('The dedicated agent-browser session is unavailable.');
  const endpoint = result.stdout.split(/\r?\n/).find(line => /^ws:\/\/127\.0\.0\.1:\d+\//.test(line));
  if (!endpoint) fail('Expected a loopback CDP endpoint for the dedicated headed browser.');
  return endpoint.trim();
}

async function jsonList(endpoint) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), popupTimeoutMs);
  let response;
  try {
    response = await fetch(new URL('/json/list', endpoint.replace(/^ws:/, 'http:')), {
      signal: controller.signal,
    });
    if (!response.ok) fail('Could not inspect the dedicated browser targets.');
    return await response.json();
  } catch (_error) {
    fail('Could not inspect the dedicated browser targets.');
  } finally {
    clearTimeout(timeout);
  }
}

async function findPopup(endpoint, previousTargetId = null) {
  const targets = await jsonList(endpoint);
  const matches = targets.filter(target => target.url === expectedPopupURL);
  if (matches.length !== 1) {
    fail('Expected exactly one actual Boopmark action popup; no popup tab fallback is allowed.');
  }
  const target = matches[0];
  if (previousTargetId && target.id !== previousTargetId) {
    fail('The actual Boopmark popup changed; refusing to control another target.');
  }
  if (!target.webSocketDebuggerUrl) fail('The actual Boopmark popup has no CDP endpoint.');
  let browserURL;
  let targetURL;
  try {
    browserURL = new URL(endpoint);
    targetURL = new URL(target.webSocketDebuggerUrl);
  } catch (_error) {
    fail('The dedicated browser returned an invalid popup CDP endpoint.');
  }
  if (browserURL.protocol !== 'ws:' || browserURL.hostname !== '127.0.0.1'
      || targetURL.protocol !== 'ws:' || targetURL.hostname !== '127.0.0.1'
      || targetURL.origin !== browserURL.origin) {
    fail('The popup CDP endpoint is not the dedicated loopback browser endpoint.');
  }
  return target;
}

function connect(webSocketURL) {
  let socket;
  try {
    socket = new WebSocket(webSocketURL);
  } catch (_error) {
    return Promise.reject(new PopupControlError('The actual Boopmark popup connection failed.'));
  }
  return new Promise((resolve, reject) => {
    let settled = false;
    const timer = setTimeout(() => {
      if (settled) return;
      settled = true;
      try { socket.close(); } catch (_error) { /* already closed */ }
      reject(new PopupControlError('The actual Boopmark popup did not become controllable.'));
    }, popupTimeoutMs);
    const rejectConnection = message => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      try { socket.close(); } catch (_error) { /* already closed */ }
      reject(new PopupControlError(message));
    };
    socket.onopen = () => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve(socket);
    };
    socket.onerror = () => rejectConnection('The actual Boopmark popup connection failed.');
    socket.onclose = () => rejectConnection('The actual Boopmark popup connection failed.');
  });
}

function call(socket, method, params = {}) {
  const id = nextMessageId++;
  return new Promise((resolve, reject) => {
    let settled = false;
    const timer = setTimeout(() => {
      if (settled) return;
      settled = true;
      reject(new PopupControlError('The actual Boopmark popup did not respond.'));
    }, popupTimeoutMs);
    const finish = callback => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      callback();
    };
    socket.onmessage = ({ data }) => {
      let message;
      try { message = JSON.parse(data); } catch (_error) { return; }
      if (message.id !== id) return;
      if (message.error) finish(() => reject(new PopupControlError('The requested popup operation failed.')));
      else finish(() => resolve(message.result || {}));
    };
    socket.onerror = () => finish(() => reject(new PopupControlError('The actual Boopmark popup connection failed.')));
    socket.onclose = () => finish(() => reject(new PopupControlError('The actual Boopmark popup connection failed.')));
    try { socket.send(JSON.stringify({ id, method, params })); }
    catch (_error) {
      finish(() => reject(new PopupControlError('The requested popup operation could not be sent.')));
    }
  });
}

async function evaluate(socket, expression) {
  const result = await call(socket, 'Runtime.evaluate', {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
  if (result.exceptionDetails || result.result?.subtype === 'error') {
    fail('The actual Boopmark popup rejected the requested operation.');
  }
  return result.result?.value;
}

async function verifyPopup(socket, endpoint, targetId) {
  await findPopup(endpoint, targetId);
  const identity = await evaluate(socket, `(async () => {
    const current = typeof globalThis.chrome?.tabs?.getCurrent === 'function'
      ? await globalThis.chrome.tabs.getCurrent()
      : null;
    return {
      href: location.href,
      extensionId: globalThis.chrome?.runtime?.id || null,
      tabsCurrent: current == null ? null : 'present',
    };
  })()`);
  if (!identity || identity.href !== expectedPopupURL || identity.extensionId !== expectedExtensionId
      || identity.tabsCurrent !== null) {
    fail('The selected target is not the actual Boopmark action popup.');
  }
}

function quoted(value) {
  return JSON.stringify(value);
}

async function snapshot(socket) {
  const result = await evaluate(socket, `(() => {
    const visible = element => Boolean(element && !element.hidden && element.getClientRects().length);
    const fields = Object.fromEntries(${quoted(safeFieldIds)}.map(id => {
      const element = document.getElementById(id);
      return [id, element ? { value: element.value, visible: visible(element), disabled: element.disabled } : null];
    }));
    const statuses = Object.fromEntries(${quoted(safeStatusIds)}.map(id => {
      const element = document.getElementById(id);
      return [id, element ? { text: element.textContent || '', visible: visible(element) } : null];
    }));
    return {
      url: location.href,
      section: document.getElementById('capture')?.hidden ? 'setup' : 'capture',
      fields,
      statuses,
      buttons: Array.from(document.querySelectorAll('#capture button')).map(button => ({
        id: button.id, text: button.textContent || '', disabled: button.disabled,
      })),
    };
  })()`);
  // `#api-key` is deliberately absent from this object and is never read.
  process.stdout.write(`${JSON.stringify(result)}\n`);
}

async function inspect(socket) {
  const result = await evaluate(socket, `(async () => {
    const visible = element => Boolean(element && !element.hidden && element.getClientRects().length);
    const fieldIds = ${quoted(safeFieldIds)};
    const statusIds = ${quoted(safeStatusIds)};
    const all = await globalThis.chrome.permissions.getAll();
    const [localState, sessionState] = await Promise.all([
      globalThis.chrome.storage.local.get(null),
      globalThis.chrome.storage.session.get(null),
    ]);
    const contains = pattern => globalThis.chrome.permissions.contains({ origins: [pattern] });
    const save = document.getElementById('save-button');
    return {
      layout: {
        innerWidth: window.innerWidth,
        scrollWidth: document.documentElement.scrollWidth,
        visibleSave: visible(save),
      },
      fieldLabels: fieldIds.map(id => {
        const label = document.querySelector('label[for="' + id + '"]');
        return { id, text: label?.textContent?.trim() || null };
      }),
      liveStatuses: statusIds.map(id => {
        const element = document.getElementById(id);
        return element ? {
          id,
          text: element.textContent || '',
          role: element.getAttribute('role'),
          ariaLive: element.getAttribute('aria-live'),
          visible: visible(element),
        } : { id, text: null, role: null, ariaLive: null, visible: false };
      }),
      focusedId: document.activeElement?.id || null,
      permissions: {
        permissions: Array.isArray(all?.permissions) ? all.permissions : [],
        origins: Array.isArray(all?.origins) ? all.origins : [],
      },
      storage: {
        settingsConfigured: Boolean(localState['boopmark.settings']
          && typeof localState['boopmark.settings'].server === 'string'
          && typeof localState['boopmark.settings'].apiKey === 'string'
          && localState['boopmark.settings'].apiKey.trim()),
        settingsValueCleared: localState['boopmark.settings'] == null,
        hasOperation: Object.prototype.hasOwnProperty.call(localState, 'boopmark.operation'),
        localKeyCount: Object.keys(localState).length,
        draftCount: Object.keys(sessionState).filter(key => key.startsWith('boopmark-draft:')).length,
        sessionKeyCount: Object.keys(sessionState).length,
      },
      contains443: await contains('https://boopmark.com:443/*'),
      contains8443: await contains('https://boopmark.com:8443/*'),
    };
  })()`);
  process.stdout.write(`${JSON.stringify(result)}\n`);
}

async function fill(socket, field, value) {
  if (!allowedFields.has(field)) fail('Only URL, title, description, and tags may be filled; API-key entry is unavailable.');
  if (typeof value !== 'string') fail('A non-secret field value is required.');
  const result = await evaluate(socket, `(() => {
    const element = document.getElementById(${quoted(field)});
    if (!element || !['INPUT', 'TEXTAREA'].includes(element.tagName)) throw new Error('field');
    if (element.hidden || !element.getClientRects().length || element.disabled || element.readOnly) {
      throw new Error('field');
    }
    element.focus();
    element.value = ${quoted(value)};
    element.dispatchEvent(new Event('input', { bubbles: true }));
    element.dispatchEvent(new Event('change', { bubbles: true }));
    return { field: ${quoted(field)}, length: element.value.length };
  })()`);
  process.stdout.write(`${JSON.stringify(result)}\n`);
}

async function press(socket, key) {
  if (key !== 'Tab' && key !== 'Shift+Tab') fail('Only Tab and Shift+Tab may be sent to the popup.');
  const modifiers = key === 'Shift+Tab' ? 8 : 0;
  const event = { key: 'Tab', code: 'Tab', modifiers, windowsVirtualKeyCode: 9, nativeVirtualKeyCode: 9 };
  await call(socket, 'Input.dispatchKeyEvent', { type: 'keyDown', ...event });
  await call(socket, 'Input.dispatchKeyEvent', { type: 'keyUp', ...event });
  const focusedId = await evaluate(socket, 'document.activeElement?.id || null');
  process.stdout.write(`${JSON.stringify({ key, focusedId })}\n`);
}

async function click(socket, button) {
  if (!buttonAllowed(button)) fail('That button is unavailable in this popup session.');
  const result = await evaluate(socket, `(() => {
    const element = document.getElementById(${quoted(button)});
    if (!element || element.tagName !== 'BUTTON') throw new Error('button');
    if (element.hidden || !element.getClientRects().length || element.disabled) throw new Error('button');
    element.click();
    return { button: ${quoted(button)}, clicked: true };
  })()`);
  process.stdout.write(`${JSON.stringify(result)}\n`);
}

async function connectFixture(socket) {
  requireFixtureSession();
  const point = await evaluate(socket, `(() => {
    const server = document.getElementById('server');
    const key = document.getElementById('api-key');
    const button = document.getElementById('connect-button');
    const available = element => Boolean(element && !element.hidden && element.getClientRects().length && !element.disabled);
    if (!available(server) || !available(key) || !available(button) || server.readOnly || key.readOnly
      || server.tagName !== 'INPUT' || key.tagName !== 'INPUT' || key.type !== 'password' || button.tagName !== 'BUTTON') throw new Error('fixture');
    // Fixed, public fixture credentials only. Never read an existing key value.
    server.value = 'http://127.0.0.1:4011';
    key.value = 'extension-fixture-key';
    for (const element of [server, key]) {
      element.dispatchEvent(new Event('input', { bubbles:true }));
      element.dispatchEvent(new Event('change', { bubbles:true }));
    }
    const rect = button.getBoundingClientRect();
    const x = rect.left + rect.width / 2, y = rect.top + rect.height / 2;
    if (x < 0 || y < 0 || x >= innerWidth || y >= innerHeight || !button.contains(document.elementFromPoint(x,y))) throw new Error('fixture');
    return { x, y };
  })()`);
  if (!point || !Number.isFinite(point.x) || !Number.isFinite(point.y)) fail('The fixture Connect button is unavailable.');
  // Native CDP input preserves the user activation needed by permissions.request.
  await call(socket, 'Input.dispatchMouseEvent', { type: 'mousePressed', ...point, button: 'left', clickCount: 1 });
  await call(socket, 'Input.dispatchMouseEvent', { type: 'mouseReleased', ...point, button: 'left', clickCount: 1 });
  process.stdout.write(`${JSON.stringify({ fixtureConnectSubmitted: true, connectionResult: 'inspect separately' })}\n`);
}

function screenshotPath(value) {
  const testResults = path.resolve(root, 'test-results');
  const requested = path.resolve(root, value || 'test-results/chrome-popup-control/popup.png');
  const relative = path.relative(testResults, requested);
  if (!relative || relative.startsWith('..' + path.sep) || path.isAbsolute(relative)) {
    fail('Screenshots must be written below the ignored test-results directory.');
  }
  if (!requested.toLowerCase().endsWith('.png')) fail('Popup screenshots must use a .png path.');
  return requested;
}

async function assertScreenshotSafe(socket) {
  const safe = await evaluate(socket, `(() => {
    const setup = document.getElementById('setup');
    const key = document.getElementById('api-key');
    const visible = element => Boolean(element && !element.hidden && element.getClientRects().length);
    return { setupVisible: visible(setup), credentialVisible: visible(key) };
  })()`);
  if (!safe || safe.setupVisible || safe.credentialVisible) {
    fail('Refusing to capture a popup while setup or credential UI is visible.');
  }
}

async function screenshot(socket, output) {
  await assertScreenshotSafe(socket);
  const result = await call(socket, 'Page.captureScreenshot', { format: 'png', fromSurface: true });
  if (typeof result.data !== 'string' || !result.data) fail('The popup screenshot was unavailable.');
  await assertScreenshotSafe(socket);
  const outputPath = screenshotPath(output);
  mkdirSync(path.dirname(outputPath), { recursive: true, mode: 0o700 });
  writeFileSync(outputPath, Buffer.from(result.data, 'base64'), { mode: 0o600 });
  process.stdout.write(`${JSON.stringify({ path: path.relative(root, outputPath) })}\n`);
}

async function accessibility(socket) {
  await assertScreenshotSafe(socket);
  const visibleIds = await evaluate(socket, `(() => {
    const capture = document.getElementById('capture');
    if (!capture || capture.hidden || !capture.getClientRects().length) throw new Error('capture');
    return ${quoted([...accessibilityIds])}.filter(id => {
      const element = document.getElementById(id);
      return element && capture.contains(element) && !element.hidden && element.getClientRects().length
        && getComputedStyle(element).visibility !== 'hidden';
    });
  })()`);
  if (!Array.isArray(visibleIds) || !visibleIds.length || visibleIds.some(id => !accessibilityIds.has(id))) fail('Capture accessibility controls are unavailable.');
  const document = await call(socket, 'DOM.getDocument', { depth: 0 });
  const observed = [];
  for (const id of visibleIds) {
    const match = await call(socket, 'DOM.querySelector', { nodeId: document.root.nodeId, selector: `#capture #${id}` });
    if (!match.nodeId) fail('Capture accessibility controls changed.');
    const { node } = await call(socket, 'DOM.describeNode', { nodeId: match.nodeId, depth: captureStatusIds.has(id) ? 1 : 0 });
    observed.push({ id, kind: 'element', backendDOMNodeId: node.backendNodeId });
    // Only direct text children of known status nodes, never input descendants.
    if (captureStatusIds.has(id)) for (const child of (node.children || []).slice(0, 8)) {
      if (child.nodeType === 3) observed.push({ id, kind: 'status-text', backendDOMNodeId: child.backendNodeId });
    }
  }
  const result = await call(socket, 'Accessibility.getFullAXTree');
  await assertScreenshotSafe(socket);
  const nodes = projectAccessibility(result.nodes, observed);
  process.stdout.write(`${JSON.stringify({ type: 'capture_accessibility', nodes })}\n`);
}

async function main() {
  const [command = 'snapshot', ...args] = process.argv.slice(2);
  if (!['snapshot', 'inspect', 'accessibility', 'fill', 'press', 'click', 'screenshot', 'connect-fixture'].includes(command)) {
    fail('Usage: snapshot | inspect | accessibility | fill <url|title|description|tags> <value> | press <Tab|Shift+Tab> | click <allowed-button-id> | screenshot [test-results/path.png] | connect-fixture (isolated local QA only)');
  }
  if (['snapshot', 'inspect', 'accessibility'].includes(command) && args.length) fail(`Usage: ${command}`);
  if (command === 'fill' && args.length < 2) fail('Usage: fill <url|title|description|tags> <value>');
  if (command === 'press' && args.length !== 1) fail('Usage: press <Tab|Shift+Tab>');
  if (command === 'click' && args.length !== 1) fail('Usage: click <capture-button-id>');
  if (command === 'screenshot' && args.length > 1) fail('Usage: screenshot [test-results/path.png]');
  if (command === 'connect-fixture') {
    if (args.length) fail('Usage: connect-fixture');
    requireFixtureSession();
  }
  if (command === 'click' && !buttonAllowed(args[0])) fail('That button is unavailable in this popup session.');

  const endpoint = cdpEndpoint();
  const target = await findPopup(endpoint);
  const socket = await connect(target.webSocketDebuggerUrl);
  try {
    await verifyPopup(socket, endpoint, target.id);
    if (command === 'snapshot') await snapshot(socket);
    else if (command === 'inspect') await inspect(socket);
    else if (command === 'accessibility') await accessibility(socket);
    else if (command === 'fill') await fill(socket, args[0], args.slice(1).join(' '));
    else if (command === 'press') await press(socket, args[0]);
    else if (command === 'click') await click(socket, args[0]);
    else if (command === 'connect-fixture') await connectFixture(socket);
    else await screenshot(socket, args[0]);
  } finally {
    try { socket.close(); } catch (_error) { /* already closed */ }
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main().catch(error => {
  process.stderr.write(`${error instanceof PopupControlError ? error.message : 'Popup control failed.'}\n`);
  process.exitCode = 1;
});
