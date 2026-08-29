const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const root = path.resolve(__dirname, '..');
const source = fs.readFileSync(path.join(root, 'popup.js'), 'utf8').replace(/^import .*;$/gm, '');
const tick = () => new Promise(resolve => setImmediate(resolve));
async function flush() { for (let i = 0; i < 8; i++) await tick(); }
function deferred() {
  let resolve;
  const promise = new Promise(r => { resolve = r; });
  return { promise, resolve };
}

// These tests exercise the real popup script, with only its DOM/Chrome message
// boundaries replaced. They supplement, not replace, the headed toolbar run.
async function setup() {
  const listeners = {};
  const elements = {};
  let change;
  let openOverride;
  let connectError;
  let saveError;
  let updateGate;
  let permissionGranted = true;
  const removed = [];
  const messages = [];
  const timers = [];
  const data = {
    connection: { connected: true, server: 'https://boopmark.com' },
    draft: {
      id: 'draft1', url: 'https://example.com', title: 'Before edit',
      description: '', tags: '', metadataStatus: 'filled',
    },
  };
  const document = { activeElement: null, getElementById: id => elements[id] };
  for (const match of fs.readFileSync(path.join(root, 'popup.html'), 'utf8').matchAll(/id="([^"]+)"/g)) {
    const id = match[1];
    elements[id] = {
      value: '', hidden: false, disabled: false,
      classList: { add() {}, remove() {}, toggle() {} },
      setAttribute() {}, removeAttribute() {},
      addEventListener(event, callback) { listeners[`${id}:${event}`] = callback; },
      focus() { document.activeElement = this; },
    };
  }
  elements.server.value = 'https://boopmark.com';
  const chrome = {
    tabs: { query: async () => [{ id: 1, url: 'https://example.com', title: 'Before edit' }] },
    permissions: {
      request: async () => permissionGranted,
      remove: async value => { removed.push(value); },
    },
    runtime: {
      onMessage: { addListener(callback) { change = callback; } },
      async sendMessage(message) {
        messages.push(message.type);
        if (message.type === 'OPEN') return openOverride ? openOverride() : structuredClone(data);
        if (message.type === 'UPDATE') {
          if (updateGate) await updateGate;
          data.draft[message.field] = message.value;
          return { ok: true };
        }
        if (message.type === 'CONNECT' && connectError) throw new Error(connectError);
        if (message.type === 'SAVE' && saveError) throw new Error(saveError);
        if (message.type === 'CANCEL' || message.type === 'ACK') change({ type: 'STATE_CHANGED' });
        return { ok: true };
      },
    },
  };
  await new vm.Script(`(async () => { ${source}\n})()`).runInNewContext({
    document, chrome, URL, BoopmarkCore: require('../core.js'), window: { close() {} },
    setTimeout(callback) { timers.push(callback); return timers.length; },
    clearTimeout(id) { timers[id - 1] = null; },
  });
  return {
    elements, data, removed, messages, timers,
    change: () => change({ type: 'STATE_CHANGED' }),
    fire: (id, event) => listeners[`${id}:${event}`]({ preventDefault() {} }),
    override: fn => { openOverride = fn; },
    failConnect: message => { connectError = message; },
    failSave: message => { saveError = message; },
    delayUpdates: promise => { updateGate = promise; },
    denyPermission: () => { permissionGranted = false; },
  };
}

test('late OPEN responses and direct renders cannot overwrite a newly typed field', async () => {
  const ui = await setup();
  const old = deferred();
  const fresh = deferred();
  let opens = 0;
  const staleSnapshot = structuredClone(ui.data);
  ui.override(() => ++opens === 1 ? old.promise : fresh.promise);
  ui.change();
  await flush();
  ui.elements.title.value = 'My intentional edit';
  ui.fire('title', 'input');
  await flush();
  ui.fire('settings-button', 'click');
  assert.equal(ui.elements.title.value, 'My intentional edit');
  old.resolve(staleSnapshot);
  await flush();
  assert.equal(ui.elements.title.value, 'My intentional edit');
  fresh.resolve(structuredClone(ui.data));
  await flush();
  assert.equal(ui.elements.title.value, 'My intentional edit');
});

test('failed replacement connection removes its newly granted origin only', async () => {
  const ui = await setup();
  ui.elements.server.value = 'https://selfhost.example';
  ui.elements['api-key'].value = 'public-test-key';
  ui.failConnect('Worker unavailable');
  await ui.fire('connection-form', 'submit');
  assert.equal(ui.removed.length, 1);
  assert.equal(ui.removed[0].origins[0], 'https://selfhost.example:443/*');
  assert.equal(ui.elements.server.disabled, false);
});

test('failed same-origin reconnect preserves current origin permission', async () => {
  const ui = await setup();
  ui.elements['api-key'].value = 'public-test-key';
  ui.failConnect('Invalid key');
  await ui.fire('connection-form', 'submit');
  assert.equal(ui.removed.length, 0);
});

test('denied host permission neither connects nor revokes an existing origin', async () => {
  const ui = await setup();
  ui.elements.server.value = 'https://selfhost.example';
  ui.denyPermission();
  await ui.fire('connection-form', 'submit');
  assert.deepEqual(ui.messages, ['OPEN']);
  assert.equal(ui.removed.length, 0);
  assert.match(ui.elements['connection-status'].textContent, /not granted/);
});

test('settings edits and link survive state refresh while API key is focused', async () => {
  const ui = await setup();
  ui.fire('settings-button', 'click');
  ui.elements.server.value = 'https://selfhost.example';
  ui.fire('server', 'input');
  ui.elements['api-key'].focus();
  ui.change();
  await flush();
  assert.equal(ui.elements.server.value, 'https://selfhost.example');
  assert.equal(ui.elements['settings-link'].href, 'https://selfhost.example/settings');
});

test('only an uncertain operation asks users to check the library', async () => {
  const ui = await setup();
  ui.data.operation = { state: 'error', error: 'Validation rejected' };
  ui.change();
  await flush();
  assert.equal(ui.elements.recovery.hidden, false);
  assert.equal(ui.elements['library-link'].hidden, true);
  assert.equal(ui.elements['ack-button'].textContent, 'Dismiss this failed save');
  assert.equal(ui.elements['save-button'].disabled, false);
  ui.data.operation = { state: 'unknown' };
  ui.change();
  await flush();
  assert.equal(ui.elements.recovery.hidden, false);
  assert.equal(ui.elements['library-link'].hidden, false);
  assert.equal(ui.elements['save-button'].disabled, true);
});

test('cancel notification cannot reopen the draft during dismissal', async () => {
  const ui = await setup();
  await ui.fire('cancel-button', 'click');
  await flush();
  assert.deepEqual(ui.messages, ['OPEN', 'CANCEL']);
});

test('successful ACK notification cannot reopen or enrich a saved capture', async () => {
  const ui = await setup();
  ui.data.operation = { state: 'success', id: 'operation1' };
  ui.change();
  await flush();
  assert.equal(ui.timers.length, 1);
  await ui.timers[0]();
  await flush();
  assert.deepEqual(ui.messages, ['OPEN', 'OPEN', 'ACK']);
});

test('disconnecting cancels a pending successful-save close timer', async () => {
  const ui = await setup();
  ui.data.operation = { state: 'success', id: 'operation1' };
  ui.change();
  await flush();
  ui.data.operation = null;
  ui.data.connection = null;
  ui.change();
  await flush();
  assert.equal(ui.timers[0], null);
});

test('embedded control characters disable save', async () => {
  const ui = await setup();
  ui.elements.url.value = 'https://exam\nple.com';
  ui.fire('url', 'input');
  assert.equal(ui.elements['save-button'].disabled, true);
  await flush();
});

test('invalid existing connection explicitly asks to reconnect', async () => {
  const ui = await setup();
  ui.data.connection.connected = false;
  ui.change();
  await flush();
  assert.equal(ui.elements['setup-heading'].textContent, 'Reconnect Boopmark');
  assert.equal(ui.elements.setup.hidden, false);
});

test('worker dispatch failure remains visible after state recovery', async () => {
  const ui = await setup();
  ui.failSave('Worker unavailable; reopen the popup to recover.');
  await ui.fire('bookmark-form', 'submit');
  assert.match(ui.elements['save-status'].textContent, /Worker unavailable/);
});

test('revoked connection cannot hide an unresolved save or its acknowledgement', async () => {
  const ui = await setup();
  ui.data.connection.connected = false;
  ui.data.operation = { state: 'unknown', id: 'operation1' };
  ui.change();
  await flush();
  assert.equal(ui.elements.capture.hidden, false);
  assert.equal(ui.elements.setup.hidden, true);
  assert.equal(ui.elements.recovery.hidden, false);
  assert.equal(ui.elements['settings-button'].textContent, 'Reconnect');
  assert.equal(ui.elements['save-button'].disabled, true);
  assert.equal(ui.elements['autofill-button'].disabled, true);
  ui.fire('settings-button', 'click');
  assert.equal(ui.elements['back-button'].hidden, false);
  ui.fire('back-button', 'click');
  assert.equal(ui.elements.capture.hidden, false);
});

test('Save dispatch does not wait for field updates that popup closure could interrupt', async () => {
  const ui = await setup();
  const gate = deferred();
  ui.delayUpdates(gate.promise);
  ui.elements.title.value = 'Latest visible title';
  ui.fire('title', 'input');
  await flush();
  const submission = ui.fire('bookmark-form', 'submit');
  await flush();
  assert.equal(ui.messages.filter(type => type === 'SAVE').length, 1);
  gate.resolve();
  await submission;
});

test('Cancel dispatches immediately and suppresses unsent field updates', async () => {
  const ui = await setup();
  const gate = deferred();
  ui.delayUpdates(gate.promise);
  ui.elements.title.value = 'An in-flight edit';
  ui.fire('title', 'input');
  await flush();
  ui.elements.description.value = 'An unsent edit';
  ui.fire('description', 'input');
  const cancellation = ui.fire('cancel-button', 'click');
  await flush();
  assert.deepEqual(ui.messages, ['OPEN', 'UPDATE', 'CANCEL']);
  gate.resolve();
  await cancellation;
  await flush();
  assert.deepEqual(ui.messages, ['OPEN', 'UPDATE', 'CANCEL']);
});
