const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const { webcrypto } = require('node:crypto');

const root = path.resolve(__dirname, '..');
const Core = require(path.join(root, 'core.js'));
const coreSource = fs.readFileSync(path.join(root, 'core.js'), 'utf8');
const apiSource = fs.readFileSync(path.join(root, 'api.js'), 'utf8');
const workerSource = fs.readFileSync(path.join(root, 'worker.js'), 'utf8')
  .replace(/^\s*import .*?;\s*$/gm, '');

const clone = value => value == null ? value : structuredClone(value);
const wait = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));
function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

function response(status, body) {
  return {
    status,
    ok: status >= 200 && status < 300,
    text: async () => body == null ? '' : JSON.stringify(body),
  };
}

function area(initial = {}, accessLevels = [], storageAccess = 'ok') {
  const values = clone(initial);
  return {
    values,
    setAccessLevel(details, callback) {
      accessLevels.push(clone(details));
      if (storageAccess === 'reject') throw new Error('storage access level failed');
      callback?.();
    },
    get(key, callback) {
      if (key == null) return callback(clone(values));
      const keys = Array.isArray(key) ? key : [key];
      const result = {};
      for (const item of keys) if (Object.prototype.hasOwnProperty.call(values, item)) result[item] = clone(values[item]);
      callback(result);
    },
    set(next, callback) {
      Object.assign(values, clone(next));
      callback?.();
    },
    remove(keys, callback) {
      for (const key of (Array.isArray(keys) ? keys : [keys])) delete values[key];
      callback?.();
    },
  };
}

function harness({
  localValues = {}, sessionValues = {}, fetchImpl, permissionGranted = true,
  permissionGrants = {}, permissionRemove = 'remove', storageAccess = 'ok',
}) {
  const accessLevels = [];
  const local = area(localValues, accessLevels, storageAccess);
  const session = area(sessionValues, accessLevels, storageAccess);
  let listener;
  const notifications = [];
  const removedPermissions = [];
  const grants = new Map(Object.entries(permissionGrants));
  let removeAttempt = 0;
  const permissionPattern = details => String(details?.origins?.[0] || '');
  const isGranted = details => {
    const pattern = permissionPattern(details);
    if (grants.has(pattern)) return grants.get(pattern) === true;
    return typeof permissionGranted === 'function' ? Boolean(permissionGranted(pattern)) : permissionGranted === true;
  };
  const chrome = {
    storage: { local, session },
    permissions: {
      contains(details, callback) { callback(isGranted(details)); },
      remove(details, callback) {
        removedPermissions.push(details);
        const selected = typeof permissionRemove === 'function'
          ? permissionRemove(details, removeAttempt++) : permissionRemove;
        if (selected === 'reject') {
          chrome.runtime.lastError = { message: 'permission removal failed' };
          callback?.();
          chrome.runtime.lastError = null;
          return;
        }
        if (selected !== 'still-granted') grants.set(permissionPattern(details), false);
        callback?.(true);
      },
    },
    runtime: {
      id: 'test-extension',
      lastError: null,
      getURL(path = '') { return `chrome-extension://test-extension/${path}`; },
      onMessage: { addListener(callback) { listener = callback; } },
      onStartup: { addListener() {} },
      onInstalled: { addListener() {} },
      sendMessage(message) { notifications.push(message); return Promise.resolve(); },
    },
  };
  const sandbox = {
    chrome,
    URL,
    fetch: fetchImpl,
    crypto: webcrypto,
    structuredClone,
    queueMicrotask,
    AbortController,
    setTimeout,
    clearTimeout,
    console,
  };
  sandbox.globalThis = sandbox;
  vm.runInNewContext(`${coreSource}\n${apiSource}\n${workerSource}`, sandbox, { filename: 'worker.js' });
  assert.equal(typeof listener, 'function', 'worker registered a message listener');
  return {
    local,
    session,
    accessLevels,
    notifications,
    removedPermissions,
    hasPermission(pattern) { return isGranted({ origins: [pattern] }); },
    send(message) {
      return new Promise(resolve => listener(message, {
        id: chrome.runtime.id,
        url: chrome.runtime.getURL('popup.html'),
      }, resolve));
    },
    sendFrom(message, senderOverrides = {}) {
      return new Promise(resolve => {
        const accepted = listener(message, {
          id: chrome.runtime.id,
          url: chrome.runtime.getURL('popup.html'),
          ...senderOverrides,
        }, resolve);
        if (accepted === false) resolve({ ignored: true });
      });
    },
  };
}

function settings(server = 'http://127.0.0.1:4011') {
  return { server, apiKey: 'extension-test-key', epoch: 'epoch-one', authError: false };
}

test('worker restricts both storage areas and ignores foreign senders', async () => {
  const calls = [];
  const ui = harness({
    localValues: { 'boopmark.settings': settings() },
    fetchImpl: async (url, init) => { calls.push({ url, init }); return response(200, []); },
  });
  assert.deepEqual(ui.accessLevels, [
    { accessLevel: 'TRUSTED_CONTEXTS' },
    { accessLevel: 'TRUSTED_CONTEXTS' },
  ]);
  const ignored = await ui.sendFrom({ type: 'OPEN', tab: {
    id: 1, url: 'https://example.com/foreign', title: 'Foreign',
  } }, { id: 'another-extension', url: 'chrome-extension://another-extension/popup.html' });
  assert.deepEqual(ignored, { ignored: true });
  assert.equal(calls.length, 0);
  assert.equal(Object.keys(ui.session.values).length, 0);
});

test('storage access-level failure fails closed before handling commands', async () => {
  const calls = [];
  const ui = harness({
    storageAccess: 'reject',
    localValues: { 'boopmark.settings': settings() },
    fetchImpl: async (url, init) => { calls.push({ url, init }); return response(200, []); },
  });
  const result = await ui.send({ type: 'OPEN', tab: {
    id: 2, url: 'https://example.com/no-storage', title: 'No storage',
  } });
  assert.equal(result.ok, false);
  assert.equal(result.error.kind, 'error');
  assert.equal(calls.length, 0);
  assert.equal(Object.keys(ui.session.values).length, 0);
});

test('empty, partial, and failed suggestions preserve fallback fields and remain saveable', async () => {
  const variants = [
    { name: 'empty', body: { title: null, description: null, tags: [] }, status: 'empty', description: '' },
    { name: 'partial', body: { title: null, description: 'Partial description', tags: [] }, status: 'filled', description: 'Partial description' },
    { name: 'error', body: null, status: 'error', description: '', suggestionStatus: 503 },
  ];
  for (const variant of variants) {
    let createCount = 0;
    const calls = [];
    const fetchImpl = async url => {
      const text = String(url);
      calls.push(text);
      if (text.endsWith('/api/v1/bookmarks/suggest')) {
        return response(variant.suggestionStatus || 200, variant.body);
      }
      if (text.endsWith('/api/v1/bookmarks')) {
        createCount += 1;
        return response(201, { id: `fallback-${variant.name}` });
      }
      throw new Error(`unexpected URL ${text}`);
    };
    const ui = harness({ localValues: { 'boopmark.settings': settings() }, fetchImpl });
    const tab = { id: 40, url: `https://example.com/fallback-${variant.name}`, title: 'Browser fallback title' };
    const opened = await ui.send({ type: 'OPEN', tab });
    await wait(30);
    const ready = await ui.send({ type: 'OPEN', tab });
    assert.equal(ready.draft.metadataStatus, variant.status, variant.name);
    assert.equal(ready.draft.title, 'Browser fallback title', `${variant.name} keeps browser title`);
    assert.equal(ready.draft.description, variant.description, variant.name);
    if (variant.name === 'error') assert.match(String(ready.draft.error), /HTTP 503|metadata/i);

    const saved = await ui.send({ type: 'SAVE', draftId: opened.draft.id, fields: {
      url: ready.draft.url, title: ready.draft.title, description: ready.draft.description, tags: ready.draft.tags,
    } });
    assert.equal(saved.ok, true, variant.name);
    assert.equal(saved.operation.state, 'pending', variant.name);
    await wait(30);
    const confirmed = await ui.send({ type: 'OPEN', tab });
    assert.equal(confirmed.operation.state, 'success', variant.name);
    assert.equal(createCount, 1, `${variant.name} saves once`);
    assert.equal(calls.filter(call => call.endsWith('/api/v1/bookmarks/suggest')).length, 1, `${variant.name} suggests once`);
  }
});

test('worker OPEN starts one automatic suggestion and SAVE sends one reviewed payload', async () => {
  const calls = [];
  const fixture = {
    id: 'fixture-bookmark-1',
    url: 'https://example.com/article?run=one#fragment',
  };
  const fetchImpl = async (url, init) => {
    calls.push({ url: String(url), init });
    if (String(url).includes('/api/v1/bookmarks?limit=1')) return response(200, []);
    if (String(url).endsWith('/api/v1/bookmarks/suggest')) {
      return response(200, { title: 'Suggested title', description: 'Suggested description', tags: ['article', 'test'] });
    }
    if (String(url).endsWith('/api/v1/bookmarks')) return response(201, fixture);
    throw new Error(`unexpected URL ${url}`);
  };
  const ui = harness({ localValues: { 'boopmark.settings': settings() }, fetchImpl });
  const tab = { id: 17, url: fixture.url, title: 'Browser fallback title' };

  const opened = await ui.send({ type: 'OPEN', tab });
  assert.equal(opened.ok, true);
  assert.equal(opened.draft.metadataStatus, 'loading');
  assert.equal(opened.draft.title, 'Browser fallback title');
  await wait(25);

  const refreshed = await ui.send({ type: 'OPEN', tab });
  assert.equal(refreshed.draft.metadataStatus, 'filled');
  assert.equal(refreshed.draft.title, 'Suggested title');
  assert.equal(calls.filter(call => call.url.endsWith('/api/v1/bookmarks/suggest')).length, 1);

  await ui.send({ type: 'UPDATE', draftId: refreshed.draft.id, field: 'title', value: 'Reviewed title' });
  await ui.send({ type: 'UPDATE', draftId: refreshed.draft.id, field: 'description', value: '' });
  await ui.send({ type: 'UPDATE', draftId: refreshed.draft.id, field: 'tags', value: 'kept, , reviewed' });
  const dispatched = await ui.send({
    type: 'SAVE',
    draftId: refreshed.draft.id,
    fields: { url: fixture.url, title: 'Reviewed title', description: '', tags: 'kept, , reviewed' },
  });
  assert.equal(dispatched.ok, true);
  assert.equal(dispatched.operation.state, 'pending');
  await wait(25);
  const saved = await ui.send({ type: 'OPEN', tab });
  assert.equal(saved.ok, true);
  assert.equal(saved.operation.state, 'success');
  assert.equal(saved.operation.bookmarkId, fixture.id);

  const creates = calls.filter(call => call.url.endsWith('/api/v1/bookmarks'));
  assert.equal(creates.length, 1);
  assert.deepEqual(JSON.parse(creates[0].init.body), {
    url: fixture.url, title: 'Reviewed title', description: '', tags: ['kept', 'reviewed'],
  });
  assert.equal(creates[0].url.includes('suggest='), false);
  assert.match(
    creates[0].init.headers['Idempotency-Key'],
    /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i,
  );
  assert.equal(ui.session.values[refreshed.draft.id], undefined, 'confirmed save clears session draft');

  const reopened = await ui.send({ type: 'OPEN', tab });
  assert.equal(reopened.operation.state, 'success');
  assert.equal(calls.filter(call => call.url.endsWith('/api/v1/bookmarks/suggest')).length, 1, 'success reopen does not suggest again');

  // A terminal success marker remains a duplicate-submit guard even if a
  // stale popup still has a draft object when it reopens.
  ui.session.values[reopened.draft.id] = clone(reopened.draft);
  const duplicate = await ui.send({ type: 'SAVE', draftId: reopened.draft.id, fields: {
    url: fixture.url, title: 'Would duplicate', description: '', tags: '',
  } });
  assert.equal(duplicate.ok, true);
  assert.equal(duplicate.operation.state, 'success');
  assert.equal(calls.filter(call => call.url.endsWith('/api/v1/bookmarks')).length, 1, 'success marker blocks a duplicate create');
});

test('worker restart recovers a pending marker as unknown without replaying create', async () => {
  const calls = [];
  const fetchImpl = async (url, init) => {
    calls.push({ url: String(url), init });
    return response(201, { id: 'must-not-be-created' });
  };
  const server = 'http://127.0.0.1:4011';
  const draftId = 'boopmark-draft:http%3A%2F%2F127.0.0.1%3A4011:19:https%3A%2F%2Fexample.com%2Fpending';
  const pending = {
    version: 1, id: 'operation-pending', state: 'pending', server, connectionEpoch: 'epoch-one',
    draftId, url: 'https://example.com/pending', submittedAt: Date.now(), error: '',
  };
  const sessionDraft = {
    id: draftId, server, connectionEpoch: 'epoch-one', tabId: 19,
    originalUrl: 'https://example.com/pending', url: 'https://example.com/pending', title: 'Pending',
    description: '', tags: '', dirty: { url: false, title: false, description: false, tags: false },
    generated: { title: false, description: false, tags: false }, browserTitle: 'Pending', generation: 1,
    suggestionAttempted: true, metadataStatus: 'filled', error: '', status: 'saving', revision: 1,
  };
  const ui = harness({
    localValues: { 'boopmark.settings': settings(server), 'boopmark.operation': pending },
    sessionValues: { [draftId]: sessionDraft }, fetchImpl,
  });
  const reopened = await ui.send({ type: 'OPEN', tab: { id: 19, url: sessionDraft.url, title: 'Pending' } });
  assert.equal(reopened.operation.state, 'unknown');
  assert.match(reopened.operation.error, /could not be confirmed/i);
  assert.equal(calls.length, 0, 'unknown operation is never replayed');
});

test('permission failure fails closed before any API request and never returns the key', async () => {
  const calls = [];
  const server = 'http://127.0.0.1:4011';
  const ui = harness({
    localValues: { 'boopmark.settings': settings(server) },
    permissionGranted: false,
    fetchImpl: async (url, init) => { calls.push({ url, init }); return response(200, []); },
  });
  const opened = await ui.send({ type: 'OPEN', tab: { id: 21, url: 'https://example.com/no-access', title: 'No access' } });
  assert.equal(opened.ok, true);
  assert.equal(opened.connection.connected, false);
  assert.equal(opened.connection.server, server);
  assert.equal(Object.prototype.hasOwnProperty.call(opened.connection, 'apiKey'), false);
  assert.equal(calls.length, 0);
  const suggested = await ui.send({ type: 'SUGGEST', draftId: opened.draft.id });
  assert.equal(suggested.ok, false);
  assert.equal(calls.length, 0);
});

test('permission-failed SAVE retains the reviewed snapshot without creating', async () => {
  const calls = [];
  const server = 'http://127.0.0.1:4011';
  const ui = harness({
    localValues: { 'boopmark.settings': settings(server) },
    permissionGranted: false,
    fetchImpl: async (url, init) => { calls.push({ url, init }); return response(201, { id: 'unexpected' }); },
  });
  const opened = await ui.send({ type: 'OPEN', tab: {
    id: 211, url: 'https://example.com/permission-save', title: 'Browser title',
  } });
  const failed = await ui.send({ type: 'SAVE', draftId: opened.draft.id, fields: {
    url: 'https://example.com/reviewed', title: '', description: 'Retained after denial', tags: 'kept, , reviewed',
  } });
  assert.equal(failed.ok, false);
  assert.equal(failed.error.kind, 'permission');
  assert.equal(calls.length, 0, 'permission failure happens before any API request');
  const retained = ui.session.values[opened.draft.id];
  assert.equal(retained.url, 'https://example.com/reviewed');
  assert.equal(retained.title, '', 'intentional title clear is retained');
  assert.equal(retained.description, 'Retained after denial');
  assert.equal(retained.tags, 'kept, reviewed');
  assert.equal(retained.dirty.title, true);
  assert.equal(retained.dirty.url, true);
});

test('unsupported capture and edited target never issue lookup or create requests', async () => {
  const calls = [];
  const ui = harness({
    localValues: { 'boopmark.settings': settings() },
    fetchImpl: async (url, init) => { calls.push({ url, init }); return response(200, []); },
  });
  const opened = await ui.send({ type: 'OPEN', tab: {
    id: 212, url: 'chrome://settings', title: 'Chrome settings',
  } });
  assert.equal(opened.ok, true);
  assert.equal(opened.draft.metadataStatus, 'empty');
  assert.equal(calls.length, 0);
  const failed = await ui.send({ type: 'SAVE', draftId: opened.draft.id, fields: {
    url: 'chrome://settings', title: 'Chrome settings', description: '', tags: '',
  } });
  assert.equal(failed.ok, false);
  assert.equal(failed.error.kind, 'validation');
  assert.equal(calls.length, 0);
});

test('a late suggestion from the previous connection cannot overwrite a rebound draft', async () => {
  const pending = [];
  const server = 'http://127.0.0.1:4011';
  const fetchImpl = async (url, init) => {
    const text = String(url);
    if (text.includes('/api/v1/bookmarks/suggest')) {
      const request = deferred();
      pending.push({ request, init });
      // Deliberately ignore AbortSignal: the generation/connection guard must
      // still reject a response that races with reconnect cancellation.
      return request.promise;
    }
    if (text.includes('/api/v1/bookmarks?limit=1')) return response(200, []);
    throw new Error(`unexpected URL ${text}`);
  };
  const ui = harness({ localValues: { 'boopmark.settings': settings(server) }, fetchImpl });
  const tab = { id: 213, url: 'https://example.com/reconnect', title: 'Original title' };
  const opened = await ui.send({ type: 'OPEN', tab });
  assert.equal(pending.length, 1);
  await ui.send({ type: 'UPDATE', draftId: opened.draft.id, field: 'title', value: 'Authored title' });

  const connected = await ui.send({ type: 'CONNECT', server, apiKey: 'replacement-key' });
  assert.equal(connected.ok, true);
  const rebound = await ui.send({ type: 'OPEN', tab });
  assert.equal(pending.length, 2, 'reconnect starts one fresh suggestion');
  pending[1].request.resolve(response(200, { title: 'Fresh title', description: 'Fresh description', tags: ['fresh'] }));
  await wait(25);
  pending[0].request.resolve(response(200, { title: 'STALE title', description: 'STALE description', tags: ['stale'] }));
  await wait(35);

  const current = await ui.send({ type: 'OPEN', tab });
  assert.equal(current.draft.title, 'Authored title', 'authored field survives reconnect and stale response');
  assert.equal(current.draft.description, 'Fresh description');
  assert.equal(current.draft.tags, 'fresh');
  assert.equal(current.draft.connectionEpoch, connected.connection.epoch);
  await ui.send({ type: 'CANCEL', draftId: current.draft.id });
});

function switchDraft(server) {
  const draft = Core.createDraft({
    server,
    connectionEpoch: 'old-epoch',
    tabId: 214,
    url: 'https://example.com/connection-switch',
    title: 'Authored before switch',
  });
  draft.description = 'Retain this draft';
  draft.tags = 'keep';
  draft.dirty.title = true;
  draft.dirty.description = true;
  draft.dirty.tags = true;
  draft.suggestionAttempted = true;
  draft.metadataStatus = 'filled';
  return draft;
}

function switchHarness({ permissionRemove }) {
  const oldServer = 'https://old.example';
  const newServer = 'https://new.example';
  const oldPattern = 'https://old.example:443/*';
  const newPattern = 'https://new.example:443/*';
  const draft = switchDraft(oldServer);
  const ui = harness({
    localValues: { 'boopmark.settings': settings(oldServer) },
    sessionValues: { [draft.id]: draft },
    permissionGranted: false,
    permissionGrants: { [oldPattern]: true, [newPattern]: true },
    permissionRemove,
    fetchImpl: async url => {
      assert.ok(String(url).startsWith(`${newServer}/`));
      return response(200, []);
    },
  });
  return { ui, oldServer, newServer, oldPattern, newPattern, draft };
}

test('server switch no-op removal preserves old identity and draft, then rolls back new grant', async () => {
  const switched = switchHarness({ permissionRemove: ({ origins }) => origins[0].startsWith('https://old.example') ? 'still-granted' : 'remove' });
  const result = await switched.ui.send({ type: 'CONNECT', server: switched.newServer, apiKey: 'new-key' });
  assert.equal(result.ok, false);
  assert.equal(result.error.kind, 'permission');
  assert.equal(switched.ui.local.values['boopmark.settings'].server, switched.oldServer);
  assert.deepEqual(switched.ui.session.values[switched.draft.id].description, 'Retain this draft');
  assert.equal(switched.ui.hasPermission(switched.oldPattern), true);
  assert.equal(switched.ui.hasPermission(switched.newPattern), false, 'new grant is rolled back');
  assert.deepEqual(switched.ui.removedPermissions.map(item => item.origins[0]), [switched.oldPattern, switched.newPattern]);
});

test('server switch removal rejection preserves old identity and draft, then rolls back new grant', async () => {
  const switched = switchHarness({ permissionRemove: ({ origins }) => origins[0].startsWith('https://old.example') ? 'reject' : 'remove' });
  const result = await switched.ui.send({ type: 'CONNECT', server: switched.newServer, apiKey: 'new-key' });
  assert.equal(result.ok, false);
  assert.equal(result.error.kind, 'error');
  assert.equal(switched.ui.local.values['boopmark.settings'].server, switched.oldServer);
  assert.equal(switched.ui.session.values[switched.draft.id].title, 'Authored before switch');
  assert.equal(switched.ui.hasPermission(switched.oldPattern), true);
  assert.equal(switched.ui.hasPermission(switched.newPattern), false, 'new grant is rolled back');
  assert.deepEqual(switched.ui.removedPermissions.map(item => item.origins[0]), [switched.oldPattern, switched.newPattern]);
});

test('successful server switch verifies old origin removal before replacing identity and drafts', async () => {
  const switched = switchHarness({ permissionRemove: 'remove' });
  const result = await switched.ui.send({ type: 'CONNECT', server: switched.newServer, apiKey: 'new-key' });
  assert.equal(result.ok, true);
  assert.equal(result.connection.server, switched.newServer);
  assert.equal(switched.ui.local.values['boopmark.settings'].server, switched.newServer);
  assert.equal(switched.ui.session.values[switched.draft.id], undefined, 'old drafts clear after removal is verified');
  assert.equal(switched.ui.hasPermission(switched.oldPattern), false);
  assert.equal(switched.ui.hasPermission(switched.newPattern), true);
  assert.deepEqual(switched.ui.removedPermissions.map(item => item.origins[0]), [switched.oldPattern]);
});

test('user edits and intentional clears survive a delayed suggestion', async () => {
  const suggestion = deferred();
  const pending = [];
  const fetchImpl = async (url, init) => {
    const text = String(url);
    if (text.includes('/api/v1/bookmarks/suggest')) {
      pending.push(init);
      return suggestion.promise;
    }
    if (text.includes('/api/v1/bookmarks?limit=1')) return response(200, []);
    throw new Error(`unexpected URL ${text}`);
  };
  const ui = harness({ localValues: { 'boopmark.settings': settings() }, fetchImpl });
  const tab = { id: 214, url: 'https://example.com/delayed-edit', title: 'Browser fallback' };
  const opened = await ui.send({ type: 'OPEN', tab });
  assert.equal(pending.length, 1);
  await ui.send({ type: 'UPDATE', draftId: opened.draft.id, field: 'title', value: 'Authored title' });
  await ui.send({ type: 'UPDATE', draftId: opened.draft.id, field: 'description', value: '' });
  suggestion.resolve(response(200, {
    title: 'Server title', description: 'Server description', tags: ['server'],
  }));
  await wait(35);
  const current = await ui.send({ type: 'OPEN', tab });
  assert.equal(current.draft.title, 'Authored title');
  assert.equal(current.draft.description, '', 'intentional clear is not refilled');
  assert.equal(current.draft.tags, 'server');
  await ui.send({ type: 'CANCEL', draftId: current.draft.id });
});

test('a delayed A suggestion cannot suppress or overwrite debounced B then A requests', async () => {
  const pending = [];
  const server = 'http://127.0.0.1:4011';
  const fetchImpl = async (url, init) => {
    const text = String(url);
    if (text.includes('/api/v1/bookmarks/suggest')) {
      const request = deferred();
      pending.push({ url: text, request, init });
      // This intentionally ignores AbortSignal: it models a server response
      // that raced with cancellation, which the generation guard must reject.
      return request.promise;
    }
    if (text.includes('/api/v1/bookmarks?limit=1')) return response(200, []);
    throw new Error(`unexpected URL ${text}`);
  };
  const ui = harness({ localValues: { 'boopmark.settings': settings(server) }, fetchImpl });
  const original = { id: 22, url: 'https://example.com/a', title: 'A browser title' };
  const opened = await ui.send({ type: 'OPEN', tab: original });
  const draftId = opened.draft.id;
  assert.equal(pending.length, 1);

  await ui.send({ type: 'UPDATE', draftId, field: 'url', value: 'https://example.com/b' });
  await wait(390);
  assert.equal(pending.length, 2, 'B starts after URL debounce even while A is unresolved');
  pending[1].request.resolve(response(200, { title: 'B title', description: 'B description', tags: ['b'] }));
  await wait(25);

  await ui.send({ type: 'UPDATE', draftId, field: 'url', value: original.url });
  await wait(390);
  assert.equal(pending.length, 3, 'returning to A gets a fresh generation');
  pending[2].request.resolve(response(200, { title: 'A2 title', description: 'A2 description', tags: ['a2'] }));
  // Resolve the stale first response after the current A2 request, then give
  // the worker queue time to process both completions.
  pending[0].request.resolve(response(200, { title: 'STALE A title', description: 'stale', tags: ['stale'] }));
  await wait(35);

  const current = await ui.send({ type: 'OPEN', tab: original });
  assert.equal(current.draft.url, original.url);
  assert.equal(current.draft.title, 'A2 title');
  assert.deepEqual(current.draft.tags, 'a2');
  assert.equal(pending.filter(item => item.url.endsWith('/api/v1/bookmarks/suggest')).length, 3);
  await ui.send({ type: 'CANCEL', draftId });
});

test('a canceled suggestion cannot overwrite a replacement capture with the same draft id', async () => {
  const pending = [];
  const server = 'http://127.0.0.1:4011';
  const fetchImpl = async (url, init) => {
    const text = String(url);
    if (text.includes('/api/v1/bookmarks/suggest')) {
      const request = deferred();
      pending.push({ request, init });
      // Ignore AbortSignal to model a response already in flight when the
      // popup is dismissed and immediately reopened for the same capture.
      return request.promise;
    }
    if (text.includes('/api/v1/bookmarks?limit=1')) return response(200, []);
    throw new Error(`unexpected URL ${text}`);
  };
  const ui = harness({ localValues: { 'boopmark.settings': settings(server) }, fetchImpl });
  const tab = { id: 28, url: 'https://example.com/cancel-reopen', title: 'Browser title' };
  const first = await ui.send({ type: 'OPEN', tab });
  assert.equal(pending.length, 1);
  await ui.send({ type: 'CANCEL', draftId: first.draft.id });

  const reopened = await ui.send({ type: 'OPEN', tab });
  assert.equal(reopened.draft.id, first.draft.id);
  assert.equal(pending.length, 2);
  pending[1].request.resolve(response(200, { title: 'NEW title', description: 'NEW description', tags: ['new'] }));
  await wait(25);
  pending[0].request.resolve(response(200, { title: 'STALE title', description: 'STALE description', tags: ['stale'] }));
  await wait(35);

  const current = await ui.send({ type: 'OPEN', tab });
  assert.equal(current.draft.title, 'NEW title');
  assert.equal(current.draft.description, 'NEW description');
  assert.equal(current.draft.tags, 'new');
  await ui.send({ type: 'CANCEL', draftId: current.draft.id });

  // Exercise the inverse completion order too: the canceled response may
  // arrive first, but must not remove or settle the replacement request.
  const secondTab = { id: 29, url: 'https://example.com/cancel-reopen-early', title: 'Second browser title' };
  const secondFirst = await ui.send({ type: 'OPEN', tab: secondTab });
  await ui.send({ type: 'CANCEL', draftId: secondFirst.draft.id });
  await ui.send({ type: 'OPEN', tab: secondTab });
  assert.equal(pending.length, 4);
  pending[2].request.resolve(response(200, { title: 'STALE early', description: 'STALE early', tags: ['stale'] }));
  await wait(25);
  pending[3].request.resolve(response(200, { title: 'FRESH late', description: 'FRESH late', tags: ['fresh'] }));
  await wait(35);
  const secondCurrent = await ui.send({ type: 'OPEN', tab: secondTab });
  assert.equal(secondCurrent.draft.title, 'FRESH late');
  assert.equal(secondCurrent.draft.description, 'FRESH late');
  assert.equal(secondCurrent.draft.tags, 'fresh');
  await ui.send({ type: 'CANCEL', draftId: secondCurrent.draft.id });
});

test('simultaneous SAVE messages dispatch one create and OPEN reports pending then success', async () => {
  const createRequest = deferred();
  let createCount = 0;
  const fetchImpl = async (url) => {
    const text = String(url);
    if (text.endsWith('/api/v1/bookmarks/suggest')) return response(200, { title: 'Suggested', description: '', tags: [] });
    if (text.endsWith('/api/v1/bookmarks')) { createCount += 1; return createRequest.promise; }
    throw new Error(`unexpected URL ${text}`);
  };
  const ui = harness({ localValues: { 'boopmark.settings': settings() }, fetchImpl });
  const tab = { id: 23, url: 'https://example.com/concurrent', title: 'Concurrent' };
  const opened = await ui.send({ type: 'OPEN', tab });
  await wait(25);
  const ready = await ui.send({ type: 'OPEN', tab });
  const fields = { url: tab.url, title: 'Reviewed', description: '', tags: '' };
  const [first, second, during] = await Promise.all([
    ui.send({ type: 'SAVE', draftId: ready.draft.id, fields }),
    ui.send({ type: 'SAVE', draftId: ready.draft.id, fields }),
    ui.send({ type: 'OPEN', tab: { ...tab, url: 'https://example.com/elsewhere', title: 'Elsewhere' } }),
  ]);
  assert.equal(first.operation.state, 'pending');
  assert.equal(second.operation.state, 'pending');
  assert.equal(second.operation.id, first.operation.id, 'concurrent saves share one durable operation');
  assert.equal(during.operation.state, 'pending');
  assert.equal(createCount, 1);
  createRequest.resolve(response(201, { id: 'concurrent-bookmark' }));
  await wait(40);
  const saved = await ui.send({ type: 'OPEN', tab });
  assert.equal(saved.operation.state, 'success');
  assert.equal(createCount, 1, 'reopening a successful operation cannot duplicate it');
});

test('definite save failure retains the draft and permits one explicit retry', async () => {
  let createCount = 0;
  const fetchImpl = async url => {
    const text = String(url);
    if (text.endsWith('/api/v1/bookmarks/suggest')) return response(200, { title: 'Suggested', tags: [] });
    if (text.endsWith('/api/v1/bookmarks')) {
      createCount += 1;
      return createCount === 1
        ? response(422, { error: 'invalid reviewed fields' })
        : response(201, { id: 'retry-bookmark' });
    }
    throw new Error(`unexpected URL ${text}`);
  };
  const ui = harness({ localValues: { 'boopmark.settings': settings() }, fetchImpl });
  const tab = { id: 30, url: 'https://example.com/definite-retry', title: 'Retry title' };
  const fields = { url: tab.url, title: 'Reviewed after failure', description: '', tags: 'retry' };
  const { ready } = await startPendingSave(ui, tab, fields);
  await wait(35);
  const failed = await ui.send({ type: 'OPEN', tab });
  assert.equal(failed.operation.state, 'error');
  assert.equal(failed.draft.status, 'ready');
  assert.equal(failed.draft.title, fields.title);
  assert.equal(createCount, 1);

  const retry = await ui.send({ type: 'SAVE', draftId: ready.draft.id, fields });
  assert.equal(retry.ok, true);
  assert.equal(retry.operation.state, 'pending');
  await wait(35);
  const saved = await ui.send({ type: 'OPEN', tab });
  assert.equal(saved.operation.state, 'success');
  assert.equal(saved.operation.bookmarkId, 'retry-bookmark');
  assert.equal(createCount, 2, 'only the explicit retry creates a second request');
});

test('unknown save outcome is retained and OPEN or ACK never resends it', async () => {
  let createCount = 0;
  const fetchImpl = async url => {
    const text = String(url);
    if (text.endsWith('/api/v1/bookmarks/suggest')) return response(200, { title: 'Suggested', tags: [] });
    if (text.endsWith('/api/v1/bookmarks')) {
      createCount += 1;
      throw new TypeError('connection lost after dispatch');
    }
    throw new Error(`unexpected URL ${text}`);
  };
  const ui = harness({ localValues: { 'boopmark.settings': settings() }, fetchImpl });
  const tab = { id: 31, url: 'https://example.com/unknown-no-replay', title: 'Unknown title' };
  const fields = { url: tab.url, title: 'Unknown reviewed', description: '', tags: '' };
  const { ready } = await startPendingSave(ui, tab, fields);
  await wait(35);
  const unknown = await ui.send({ type: 'OPEN', tab });
  assert.equal(unknown.operation.state, 'unknown');
  assert.equal(unknown.draft.title, fields.title);
  assert.equal(createCount, 1);

  const acknowledged = await ui.send({ type: 'ACK', operationId: unknown.operation.id });
  assert.equal(acknowledged.ok, true);
  assert.equal(acknowledged.operation, null);
  assert.ok(acknowledged.draft, 'acknowledging status does not discard the draft');
  const reopened = await ui.send({ type: 'OPEN', tab });
  assert.equal(reopened.operation, null);
  assert.equal(reopened.draft.title, fields.title);
  assert.equal(createCount, 1, 'OPEN and ACK are status controls, never implicit retries');
  await ui.send({ type: 'CANCEL', draftId: ready.draft.id });
});

test('unknown operation reconstructed from an edited URL blocks another save after draft loss', async () => {
  const createRequest = deferred();
  const server = 'http://127.0.0.1:4011';
  const calls = [];
  const fetchImpl = async (url) => {
    calls.push(String(url));
    if (String(url).endsWith('/api/v1/bookmarks')) return createRequest.promise;
    throw new Error(`unexpected URL ${url}`);
  };
  const first = harness({ localValues: { 'boopmark.settings': settings(server) }, fetchImpl });
  const original = { id: 24, url: 'https://example.com/original', title: 'Original' };
  const opened = await first.send({ type: 'OPEN', tab: original });
  // The initial suggestion is intentionally absent; update the URL after the
  // fresh OPEN and dispatch a reviewed edited URL.
  await first.send({ type: 'UPDATE', draftId: opened.draft.id, field: 'url', value: 'https://example.com/edited' });
  const dispatched = await first.send({ type: 'SAVE', draftId: opened.draft.id, fields: {
    url: 'https://example.com/edited', title: 'Edited', description: '', tags: '',
  } });
  assert.equal(dispatched.operation.state, 'pending');
  const persistedLocal = clone(first.local.values);
  const callsBeforeRestart = calls.length;
  const restarted = harness({ localValues: persistedLocal, sessionValues: {}, fetchImpl: async url => {
    calls.push(String(url));
    return response(201, { id: 'must-not-create' });
  } });
  const recovered = await restarted.send({ type: 'OPEN', tab: original });
  assert.equal(recovered.operation.state, 'unknown');
  assert.equal(recovered.draft.url, 'https://example.com/edited');
  const blocked = await restarted.send({ type: 'SAVE', draftId: recovered.draft.id, fields: {
    url: recovered.draft.url, title: 'Again', description: '', tags: '',
  } });
  assert.equal(blocked.operation.state, 'unknown');
  assert.equal(calls.filter(url => url.endsWith('/api/v1/bookmarks')).length, 1, 'restart does not replay or duplicate create');
  assert.equal(calls.length, callsBeforeRestart, 'blocked recovery performs no additional request');
  createRequest.resolve(response(201, { id: 'late-old-worker' }));
  await wait(30);
});

test('disconnect during an in-flight create clears the marker and late completion cannot recreate it', async () => {
  const createRequest = deferred();
  let createCount = 0;
  const fetchImpl = async url => {
    const text = String(url);
    if (text.endsWith('/api/v1/bookmarks/suggest')) return response(200, { title: 'Suggested', tags: [] });
    if (text.endsWith('/api/v1/bookmarks')) { createCount += 1; return createRequest.promise; }
    throw new Error(`unexpected URL ${text}`);
  };
  const ui = harness({ localValues: { 'boopmark.settings': settings() }, fetchImpl });
  const tab = { id: 25, url: 'https://example.com/disconnect', title: 'Disconnect' };
  const opened = await ui.send({ type: 'OPEN', tab });
  await wait(20);
  const ready = await ui.send({ type: 'OPEN', tab });
  const dispatched = await ui.send({ type: 'SAVE', draftId: ready.draft.id, fields: {
    url: tab.url, title: 'Disconnect', description: '', tags: '',
  } });
  assert.equal(dispatched.operation.state, 'pending');
  await ui.send({ type: 'DISCONNECT' });
  createRequest.resolve(response(201, { id: 'late-disconnect' }));
  await wait(40);
  assert.equal(createCount, 1);
  assert.equal(ui.local.values['boopmark.settings'], null);
  assert.equal(ui.local.values['boopmark.operation'], undefined);
});

async function startPendingSave(ui, tab, fields) {
  const opened = await ui.send({ type: 'OPEN', tab });
  await wait(20);
  const ready = await ui.send({ type: 'OPEN', tab });
  const dispatched = await ui.send({ type: 'SAVE', draftId: ready.draft.id, fields });
  assert.equal(dispatched.ok, true);
  assert.equal(dispatched.operation.state, 'pending');
  return { opened, ready, dispatched };
}

test('disconnect removal rejection preserves connection, draft, and pending operation for retry', async () => {
  const createRequest = deferred();
  let createCount = 0;
  const server = 'http://127.0.0.1:4011';
  const ui = harness({
    localValues: { 'boopmark.settings': settings(server) },
    permissionRemove: (_details, attempt) => attempt === 0 ? 'reject' : 'remove',
    fetchImpl: async url => {
      const text = String(url);
      if (text.endsWith('/api/v1/bookmarks/suggest')) return response(200, { title: 'Suggested', tags: [] });
      if (text.endsWith('/api/v1/bookmarks')) { createCount += 1; return createRequest.promise; }
      throw new Error(`unexpected URL ${text}`);
    },
  });
  const tab = { id: 26, url: 'https://example.com/disconnect-reject', title: 'Disconnect reject' };
  const fields = { url: tab.url, title: 'Reviewed', description: '', tags: '' };
  const { ready } = await startPendingSave(ui, tab, fields);
  const beforeSettings = clone(ui.local.values['boopmark.settings']);

  const failed = await ui.send({ type: 'DISCONNECT' });
  assert.equal(failed.ok, false);
  assert.equal(failed.error.kind, 'error');
  assert.deepEqual(ui.local.values['boopmark.settings'], beforeSettings, 'identity remains until removal is verified');
  assert.ok(ui.local.values['boopmark.operation'], 'pending operation remains recoverable');
  assert.equal(ui.session.values[ready.draft.id].status, 'saving');

  const disconnected = await ui.send({ type: 'DISCONNECT' });
  assert.equal(disconnected.ok, true);
  assert.equal(ui.local.values['boopmark.settings'], null);
  assert.equal(ui.local.values['boopmark.operation'], undefined);
  assert.equal(ui.session.values[ready.draft.id], undefined);
  assert.equal(createCount, 1);
  createRequest.resolve(response(201, { id: 'late-after-retry' }));
  await wait(35);
  assert.equal(ui.local.values['boopmark.operation'], undefined, 'late completion cannot recreate cleared state');
});

test('disconnect that leaves a grant present preserves state and retries after removal succeeds', async () => {
  const createRequest = deferred();
  const server = 'http://127.0.0.1:4011';
  const ui = harness({
    localValues: { 'boopmark.settings': settings(server) },
    permissionRemove: (_details, attempt) => attempt === 0 ? 'still-granted' : 'remove',
    fetchImpl: async url => {
      const text = String(url);
      if (text.endsWith('/api/v1/bookmarks/suggest')) return response(200, { title: 'Suggested', tags: [] });
      if (text.endsWith('/api/v1/bookmarks')) return createRequest.promise;
      throw new Error(`unexpected URL ${text}`);
    },
  });
  const tab = { id: 27, url: 'https://example.com/disconnect-still-granted', title: 'Disconnect still granted' };
  const fields = { url: tab.url, title: 'Reviewed', description: '', tags: '' };
  const { ready } = await startPendingSave(ui, tab, fields);

  const failed = await ui.send({ type: 'DISCONNECT' });
  assert.equal(failed.ok, false);
  assert.equal(failed.error.kind, 'permission');
  assert.ok(ui.local.values['boopmark.settings']);
  assert.ok(ui.local.values['boopmark.operation']);
  assert.equal(ui.session.values[ready.draft.id].status, 'saving');

  const disconnected = await ui.send({ type: 'DISCONNECT' });
  assert.equal(disconnected.ok, true);
  assert.equal(ui.local.values['boopmark.settings'], null);
  assert.equal(ui.local.values['boopmark.operation'], undefined);
  assert.equal(ui.session.values[ready.draft.id], undefined);
  createRequest.resolve(response(201, { id: 'late-after-grant-retry' }));
  await wait(35);
  assert.equal(ui.local.values['boopmark.operation'], undefined);
});

test('popup close and reopen retains the session draft without a second suggestion or create', async () => {
  const suggestion = deferred();
  const calls = [];
  const fetchImpl = async (url, init) => {
    const text = String(url);
    calls.push({ url: text, init });
    if (text.endsWith('/api/v1/bookmarks/suggest')) return suggestion.promise;
    throw new Error(`unexpected URL ${text}`);
  };
  const ui = harness({ localValues: { 'boopmark.settings': settings() }, fetchImpl });
  const tab = { id: 32, url: 'https://example.com/outside-dismiss', title: 'Browser title' };

  const first = await ui.send({ type: 'OPEN', tab });
  await ui.send({ type: 'UPDATE', draftId: first.draft.id, field: 'title', value: 'Authored while loading' });

  // Closing an action popup does not send CANCEL. Reopening the same capture
  // must find the session draft and keep the in-flight request associated with
  // it, rather than starting another automatic request.
  const reopenedWhileLoading = await ui.send({ type: 'OPEN', tab });
  assert.equal(reopenedWhileLoading.draft.id, first.draft.id);
  assert.equal(reopenedWhileLoading.draft.title, 'Authored while loading');
  assert.equal(reopenedWhileLoading.draft.metadataStatus, 'loading');
  assert.equal(calls.filter(call => call.url.endsWith('/api/v1/bookmarks/suggest')).length, 1);

  suggestion.resolve(response(200, {
    title: 'Late server title', description: 'Fresh description', tags: ['fresh'],
  }));
  await wait(35);
  const ready = await ui.send({ type: 'OPEN', tab });
  assert.equal(ready.draft.title, 'Authored while loading');
  assert.equal(ready.draft.description, 'Fresh description');
  assert.equal(ready.draft.tags, 'fresh');
  assert.equal(calls.filter(call => call.url.endsWith('/api/v1/bookmarks')).length, 0);
  await ui.send({ type: 'CANCEL', draftId: ready.draft.id });
});

test('revoked credentials stop enrichment and saving without an empty-result claim', async () => {
  const calls = [];
  const server = 'http://127.0.0.1:4011';
  const fetchImpl = async (url, init) => {
    calls.push({ url: String(url), init });
    return response(401, { error: 'Fixture credentials revoked' });
  };
  const ui = harness({ localValues: { 'boopmark.settings': settings(server) }, fetchImpl });
  const tab = { id: 33, url: 'https://example.com/revoked', title: 'Browser fallback title' };

  const opened = await ui.send({ type: 'OPEN', tab });
  await wait(35);
  const failed = await ui.send({ type: 'OPEN', tab });
  assert.equal(failed.connection.connected, false);
  assert.equal(failed.connection.error.kind, 'auth');
  assert.equal(failed.draft.title, 'Browser fallback title');
  assert.equal(failed.draft.metadataStatus, 'error');
  assert.match(String(failed.draft.error), /Reconnect|credential|HTTP 401/i);
  assert.equal(calls.filter(call => call.url.endsWith('/api/v1/bookmarks/suggest')).length, 1);

  const blocked = await ui.send({ type: 'SAVE', draftId: opened.draft.id, fields: {
    url: tab.url, title: failed.draft.title, description: '', tags: '',
  } });
  assert.equal(blocked.ok, false);
  assert.equal(blocked.error.kind, 'auth');
  assert.equal(calls.filter(call => call.url.endsWith('/api/v1/bookmarks')).length, 0);
  assert.equal(ui.local.values['boopmark.settings'].authError, true);
});

test('worker startup collects no tab data or network data before popup invocation', async () => {
  const calls = [];
  const ui = harness({
    fetchImpl: async (url, init) => {
      calls.push({ url: String(url), init });
      throw new Error('network must not start before OPEN');
    },
  });
  await wait(30);
  assert.equal(calls.length, 0);
  assert.deepEqual(ui.local.values, {});
  assert.deepEqual(ui.session.values, {});
});

test('disconnect clears credentials, operation markers, and session drafts after verifying grant removal', async () => {
  const server = 'http://127.0.0.1:4011';
  const draft = Core.createDraft({
    server, connectionEpoch: 'epoch-one', tabId: 34,
    url: 'https://example.com/disconnect-cleanup', title: 'Draft to clear',
  });
  const marker = {
    version: 1,
    id: '4d8c0f1b-6e7a-4d5f-9a2b-1c3e5f7a9b0d',
    state: 'unknown',
    server,
    connectionEpoch: 'epoch-one',
    draftId: draft.id,
    url: draft.url,
    submittedAt: Date.now(),
    error: 'Save could not be confirmed.',
  };
  const calls = [];
  const ui = harness({
    localValues: { 'boopmark.settings': settings(server), 'boopmark.operation': marker },
    sessionValues: { [draft.id]: draft, unrelated: { keep: true } },
    fetchImpl: async (url, init) => {
      calls.push({ url: String(url), init });
      throw new Error('disconnect must not issue a network request');
    },
  });

  const disconnected = await ui.send({ type: 'DISCONNECT' });
  assert.equal(disconnected.ok, true);
  assert.equal(disconnected.connection.connected, false);
  assert.equal(ui.local.values['boopmark.settings'], null);
  assert.equal(ui.local.values['boopmark.operation'], undefined);
  assert.equal(ui.session.values[draft.id], undefined);
  assert.deepEqual(ui.session.values.unrelated, { keep: true });
  assert.equal(ui.hasPermission('http://127.0.0.1:4011/*'), false);
  assert.deepEqual(ui.removedPermissions.map(item => item.origins[0]), ['http://127.0.0.1:4011/*']);
  assert.equal(calls.length, 0);
});

test('browser restart with a pending save marker reconstructs unknown state and never replays it', async () => {
  const server = 'http://127.0.0.1:4011';
  const draftId = 'boopmark-draft:http%3A%2F%2F127.0.0.1%3A4011:35:https%3A%2F%2Fexample.com%2Frestart';
  const pending = {
    version: 1,
    id: '4d8c0f1b-6e7a-4d5f-9a2b-1c3e5f7a9b0d',
    state: 'pending',
    server,
    connectionEpoch: 'epoch-one',
    draftId,
    url: 'https://example.com/restart?run=local#fragment',
    submittedAt: Date.now(),
    error: '',
  };
  const calls = [];
  const ui = harness({
    localValues: { 'boopmark.settings': settings(server), 'boopmark.operation': pending },
    // Browser restart clears session storage; the durable operation marker is
    // enough to recover the reviewed URL and block an implicit retry.
    fetchImpl: async (url, init) => {
      calls.push({ url: String(url), init });
      return response(201, { id: 'must-not-be-created' });
    },
  });

  const reopened = await ui.send({ type: 'OPEN', tab: {
    id: 35, url: 'https://example.com/restart?run=local#fragment', title: 'Restarted',
  } });
  assert.equal(reopened.operation.state, 'unknown');
  assert.equal(reopened.draft.url, pending.url);
  assert.match(reopened.operation.error, /could not be confirmed/i);
  assert.equal(calls.length, 0);

  const blocked = await ui.send({ type: 'SAVE', draftId, fields: {
    url: pending.url, title: 'Explicit retry must be deliberate', description: '', tags: '',
  } });
  assert.equal(blocked.ok, true);
  assert.equal(blocked.operation.state, 'unknown');
  assert.equal(calls.length, 0, 'reopening/reviewing an unknown operation never replays create');
});
