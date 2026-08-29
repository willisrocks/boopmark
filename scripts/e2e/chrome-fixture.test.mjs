import assert from 'node:assert/strict';
import { once } from 'node:events';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import test from 'node:test';
import { chromium } from 'playwright';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const fixturePath = path.join(root, 'scripts/e2e/chrome-fixture.mjs');
const fixtureKey = 'extension-fixture-key';

const wait = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));

async function startFixture() {
  const child = spawn(process.execPath, [fixturePath], {
    cwd: root,
    env: { ...process.env, CHROME_FIXTURE_PORT: '0' },
    stdio: ['ignore', 'pipe', 'ignore'],
  });
  let output = '';
  let settled = false;
  let timer;
  const port = await new Promise((resolve, reject) => {
    const finish = (callback, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      callback(value);
    };
    timer = setTimeout(() => {
      child.kill('SIGTERM');
      finish(reject, new Error('Fixture did not start.'));
    }, 5_000);
    child.stdout.on('data', chunk => {
      output += chunk.toString();
      const match = output.match(/Extension fixture listening at http:\/\/127\.0\.0\.1:(\d+)/);
      if (match) finish(resolve, Number(match[1]));
    });
    child.once('error', error => finish(reject, error));
    child.once('exit', code => {
      if (!settled) finish(reject, new Error(`Fixture exited before startup (${code ?? 'unknown'}).`));
    });
  });
  const base = `http://127.0.0.1:${port}`;
  return {
    base,
    child,
    async close() {
      if (child.exitCode !== null) return;
      child.kill('SIGTERM');
      await Promise.race([once(child, 'exit'), wait(2_000)]);
      if (child.exitCode === null) child.kill('SIGKILL');
    },
  };
}

async function fixtureRequest(base, endpoint, {
  method = 'GET', payload, authorized = false, idempotencyKey,
} = {}) {
  const headers = {};
  if (payload !== undefined) headers['Content-Type'] = 'application/json';
  if (authorized) headers.Authorization = `Bearer ${fixtureKey}`;
  if (idempotencyKey) headers['Idempotency-Key'] = idempotencyKey;
  return fetch(`${base}${endpoint}`, {
    method,
    headers,
    body: payload === undefined ? undefined : JSON.stringify(payload),
  });
}

async function json(response) {
  return response.json();
}

async function control(base, command) {
  const response = await fixtureRequest(base, '/__control', { method: 'POST', payload: command });
  return { response, body: await json(response) };
}

async function state(base) {
  const response = await fixtureRequest(base, '/__state');
  assert.equal(response.status, 200);
  return json(response);
}

async function waitForRequest(base, predicate) {
  const deadline = Date.now() + 2_000;
  while (Date.now() < deadline) {
    const current = await state(base);
    if (current.requests.some(predicate)) return current;
    await wait(10);
  }
  throw new Error('Timed out waiting for fixture request metadata.');
}

test('fixture counts unauthorized API requests and validates controls atomically', async t => {
  const fixture = await startFixture();
  t.after(() => fixture.close());

  let current = await state(fixture.base);
  assert.equal(current.mode, 'normal');
  assert.equal(current.suggestionDelayMs, 1_500);
  assert.equal(current.saveDelayMs, 2_500);

  const unauthorized = await fixtureRequest(fixture.base, '/api/v1/bookmarks?limit=1');
  assert.equal(unauthorized.status, 401);
  current = await state(fixture.base);
  const unauthorizedLog = current.requests.at(-1);
  assert.deepEqual(unauthorizedLog, {
    method: 'GET', path: '/api/v1/bookmarks', status: 401,
  });
  assert.equal(Object.hasOwn(unauthorizedLog, 'authorization'), false);

  const suggested = await fixtureRequest(fixture.base, '/api/v1/bookmarks/suggest', {
    method: 'POST', authorized: true, payload: { url: 'https://example.com/article' },
  });
  assert.equal(suggested.status, 200);
  const created = await fixtureRequest(fixture.base, '/api/v1/bookmarks', {
    method: 'POST', authorized: true,
    payload: { url: 'https://example.com/article', title: 'Fixture', description: '', tags: [] },
  });
  assert.equal(created.status, 201);
  const beforeInvalidControl = await state(fixture.base);

  const invalidMode = await control(fixture.base, { reset: true, mode: 'not-a-fixture-mode' });
  assert.equal(invalidMode.response.status, 400);
  const afterInvalidMode = await state(fixture.base);
  assert.equal(afterInvalidMode.mode, beforeInvalidControl.mode);
  assert.deepEqual(afterInvalidMode.events, beforeInvalidControl.events);
  assert.deepEqual(afterInvalidMode.requests, beforeInvalidControl.requests);
  assert.deepEqual(afterInvalidMode.bookmarks, beforeInvalidControl.bookmarks);

  const invalidDelay = await control(fixture.base, { reset: true, suggestionDelayMs: 15_001 });
  assert.equal(invalidDelay.response.status, 400);
  const afterInvalidDelay = await state(fixture.base);
  assert.deepEqual(afterInvalidDelay.events, beforeInvalidControl.events);
  assert.deepEqual(afterInvalidDelay.requests, beforeInvalidControl.requests);
  assert.deepEqual(afterInvalidDelay.bookmarks, beforeInvalidControl.bookmarks);

  const reset = await control(fixture.base, { reset: true, mode: 'normal' });
  assert.equal(reset.response.status, 200);
  const afterReset = await state(fixture.base);
  assert.deepEqual(afterReset.events, []);
  assert.deepEqual(afterReset.requests, []);
  assert.deepEqual(afterReset.bookmarks, []);
});

test('fixture captures slow mode and delay when each request is dispatched', async t => {
  const fixture = await startFixture();
  t.after(() => fixture.close());
  await control(fixture.base, { reset: true, mode: 'slow-suggest', suggestionDelayMs: 180 });

  const suggestStarted = Date.now();
  const pendingSuggestion = fixtureRequest(fixture.base, '/api/v1/bookmarks/suggest', {
    method: 'POST', authorized: true, payload: { url: 'https://example.com/slow-suggest' },
  });
  await waitForRequest(fixture.base, request => request.path === '/api/v1/bookmarks/suggest' && request.pending);
  await control(fixture.base, { mode: 'suggest-error', suggestionDelayMs: 0 });
  const suggestion = await pendingSuggestion;
  assert.equal(suggestion.status, 200);
  assert.ok(Date.now() - suggestStarted >= 140);
  const suggestionState = await state(fixture.base);
  assert.equal(suggestionState.requests.at(-1).status, 200);

  await control(fixture.base, { mode: 'slow-save', saveDelayMs: 180 });
  const saveStarted = Date.now();
  const pendingSave = fixtureRequest(fixture.base, '/api/v1/bookmarks', {
    method: 'POST', authorized: true,
    payload: { url: 'https://example.com/slow-save', title: '', description: '', tags: [] },
  });
  await waitForRequest(fixture.base, request => request.path === '/api/v1/bookmarks' && request.pending);
  await control(fixture.base, { mode: 'save-error', saveDelayMs: 0 });
  const save = await pendingSave;
  assert.equal(save.status, 201);
  assert.ok(Date.now() - saveStarted >= 140);
  const saveState = await state(fixture.base);
  assert.equal(saveState.requests.at(-1).status, 201);
  assert.equal(saveState.bookmarks.length, 1);
});

test('fixture preserves existing error, fallback, revoked, and unknown modes', async t => {
  const fixture = await startFixture();
  t.after(() => fixture.close());
  const suggestPayload = { url: 'https://example.com/modes' };

  await control(fixture.base, { reset: true, mode: 'suggest-error' });
  let response = await fixtureRequest(fixture.base, '/api/v1/bookmarks/suggest', {
    method: 'POST', authorized: true, payload: suggestPayload,
  });
  assert.equal(response.status, 503);

  await control(fixture.base, { mode: 'empty' });
  response = await fixtureRequest(fixture.base, '/api/v1/bookmarks/suggest', {
    method: 'POST', authorized: true, payload: suggestPayload,
  });
  assert.equal(response.status, 200);
  assert.deepEqual((await json(response)).tags, []);

  await control(fixture.base, { mode: 'partial' });
  response = await fixtureRequest(fixture.base, '/api/v1/bookmarks/suggest', {
    method: 'POST', authorized: true, payload: suggestPayload,
  });
  assert.equal(response.status, 200);
  assert.equal((await json(response)).description, 'Partial description');

  await control(fixture.base, { mode: 'revoked' });
  response = await fixtureRequest(fixture.base, '/api/v1/bookmarks/suggest', {
    method: 'POST', authorized: true, payload: suggestPayload,
  });
  assert.equal(response.status, 401);

  await control(fixture.base, { mode: 'save-error' });
  response = await fixtureRequest(fixture.base, '/api/v1/bookmarks', {
    method: 'POST', authorized: true,
    payload: { url: 'https://example.com/error', title: '', description: '', tags: [] },
  });
  assert.equal(response.status, 422);

  await control(fixture.base, { mode: 'save-unknown' });
  await assert.rejects(() => fixtureRequest(fixture.base, '/api/v1/bookmarks', {
    method: 'POST', authorized: true,
    payload: { url: 'https://example.com/unknown', title: '', description: '', tags: [] },
  }));
  const current = await state(fixture.base);
  const unknownRequest = current.requests.at(-1);
  assert.equal(unknownRequest.path, '/api/v1/bookmarks');
  assert.equal(unknownRequest.aborted, true);
  assert.equal(current.bookmarks.at(-1).url, 'https://example.com/unknown');
});

test('save-unknown transport replay deduplicates by UUID and reconciles the original record', async t => {
  const fixture = await startFixture();
  t.after(() => fixture.close());
  await control(fixture.base, { reset: true, mode: 'save-unknown' });

  const browser = await chromium.launch({ headless: true });
  t.after(() => browser.close());
  const page = await browser.newPage();
  await page.goto(`${fixture.base}/article`);
  const key = '4d8c0f1b-6e7a-4d5f-9a2b-1c3e5f7a9b0d';
  const payload = {
    url: `${fixture.base}/article?transport-replay=one#captured`,
    title: 'Reviewed once',
    description: '',
    tags: ['fixture', 'replay'],
  };
  const result = await page.evaluate(async ({ base, key, payload, fixtureKey }) => {
    try {
      const response = await fetch(`${base}/api/v1/bookmarks`, {
        method: 'POST',
        headers: {
          Authorization: `Bearer ${fixtureKey}`,
          'Content-Type': 'application/json',
          'Idempotency-Key': key,
        },
        body: JSON.stringify(payload),
      });
      return { status: response.status, body: await response.json() };
    } catch (error) {
      return { error: error?.name || 'Error' };
    }
  }, { base: fixture.base, key, payload, fixtureKey });

  assert.equal(result.status, 201, `Chromium should reconcile the replay: ${result.error || ''}`);
  const deadline = Date.now() + 2_000;
  let current;
  while (Date.now() < deadline) {
    current = await state(fixture.base);
    if (current.requests.filter(request => request.path === '/api/v1/bookmarks').length >= 2) break;
    await wait(10);
  }
  assert.ok(current, 'fixture state should be readable');
  const creates = current.requests.filter(request => request.path === '/api/v1/bookmarks');
  assert.equal(creates.length, 2, 'the browser transport should make two wire attempts');
  assert.equal(creates.filter(request => request.aborted).length, 1);
  assert.equal(creates.filter(request => request.status === 201).length, 1);
  assert.deepEqual(creates.map(request => request.idempotencyGroup), ['operation-1', 'operation-1']);
  assert.equal(current.bookmarks.length, 1, 'the replay must not create a second row');
  assert.equal(result.body.id, current.bookmarks[0].id);

  const replay = await fixtureRequest(fixture.base, '/api/v1/bookmarks', {
    method: 'POST',
    authorized: true,
    idempotencyKey: key,
    payload,
  });
  assert.equal(replay.status, 201);
  assert.equal((await json(replay)).id, current.bookmarks[0].id);

  const conflict = await fetch(`${fixture.base}/api/v1/bookmarks`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${fixtureKey}`,
      'Content-Type': 'application/json',
      'Idempotency-Key': key,
    },
    body: JSON.stringify({ ...payload, title: 'Changed payload' }),
  });
  assert.equal(conflict.status, 409);
  assert.equal((await state(fixture.base)).bookmarks.length, 1);
});

test('fixture slow modes delay only their matching API operation', async t => {
  const fixture = await startFixture();
  t.after(() => fixture.close());
  for (const [mode, endpoint] of [
    ['slow-suggest', '/api/v1/bookmarks'],
    ['slow-save', '/api/v1/bookmarks/suggest'],
  ]) {
    await control(fixture.base, { mode, suggestionDelayMs: 5_000, saveDelayMs: 5_000 });
    const response = await fetch(`${fixture.base}${endpoint}`, {
      method: 'POST',
      headers: { Authorization: `Bearer ${fixtureKey}`, 'Content-Type': 'application/json' },
      body: JSON.stringify({ url: 'https://example.com/independent-delay', title: '', description: '', tags: [] }),
      signal: AbortSignal.timeout(2_000),
    });
    assert.equal(response.status, endpoint.endsWith('/suggest') ? 200 : 201);
  }
});
