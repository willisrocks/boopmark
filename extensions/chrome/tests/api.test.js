const test = require('node:test');
const assert = require('node:assert/strict');
const Core = require('../core.js');
const { ApiClient, ApiError } = require('../api.js');

function response(status, body) {
  return new Response(body == null ? '' : JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

test('validation uses read-only list request and never exposes the key in a URL', async () => {
  const calls = [];
  const client = new ApiClient({
    server: 'https://boopmark.com/', apiKey: 'secret-key',
    fetchImpl: async (url, init) => {
      calls.push({ url, init });
      return response(200, []);
    },
  });
  await client.validateConnection();
  assert.equal(calls[0].url, 'https://boopmark.com/api/v1/bookmarks?limit=1');
  assert.equal(calls[0].init.credentials, 'omit');
  assert.equal(calls[0].init.redirect, 'error');
  assert.equal(calls[0].init.headers.Authorization, 'Bearer secret-key');
  assert.equal(calls[0].url.includes('secret-key'), false);
});

test('suggest sends only the captured URL, including query and fragment', async () => {
  const calls = [];
  const client = new ApiClient({
    server: 'https://boopmark.com', apiKey: 'key',
    fetchImpl: async (url, init) => {
      calls.push({ url, init });
      return response(200, { title: 'Title', description: null, tags: [], image_url: 'https://img.test/x' });
    },
  });
  const captured = 'https://example.com/article?fixture=one#fragment';
  const suggestion = await client.suggest(captured);
  assert.equal(JSON.parse(calls[0].init.body).url, captured);
  assert.equal(suggestion.imageUrl, 'https://img.test/x');
  assert.equal(calls[0].url, 'https://boopmark.com/api/v1/bookmarks/suggest');
});

test('create sends reviewed values exactly once without suggest query', async () => {
  const calls = [];
  const client = new ApiClient({
    server: 'https://boopmark.com', apiKey: 'key',
    fetchImpl: async (url, init) => {
      calls.push({ url, init });
      return response(201, { id: 'bookmark-1', url: 'https://example.com/a' });
    },
  });
  await client.create({ url: 'https://example.com/a?x=1#f', title: '', description: '', tags: 'a, , b' });
  assert.equal(calls.length, 1);
  assert.equal(calls[0].url, 'https://boopmark.com/api/v1/bookmarks');
  assert.deepEqual(JSON.parse(calls[0].init.body), {
    url: 'https://example.com/a?x=1#f', title: '', description: '', tags: ['a', 'b']
  });
  assert.equal(calls[0].url.includes('suggest'), false);
});

test('create requires HTTP 201 and a bookmark id', async () => {
  for (const result of [response(200, { id: 'bookmark-1' }), response(201, {}), response(201, null)]) {
    const client = new ApiClient({ server: 'https://boopmark.com', apiKey: 'key', fetchImpl: async () => result });
    await assert.rejects(client.create({ url: 'https://example.com', title: '', description: '', tags: [] }), (error) => {
      assert.ok(error instanceof ApiError);
      assert.equal(error.kind, 'unknown');
      return true;
    });
  }
});

test('save network interruptions, redirects, and server 5xx are unknown', async () => {
  for (const fetchImpl of [
    async () => { throw new TypeError('offline'); },
    async () => response(500, { error: 'server failed' }),
  ]) {
    const client = new ApiClient({ server: 'https://boopmark.com', apiKey: 'key', fetchImpl });
    await assert.rejects(client.create({ url: 'https://example.com', title: '', description: '', tags: [] }), (error) => {
      assert.equal(error.kind, 'unknown');
      return true;
    });
  }
});

test('authentication failures are distinct from empty suggestion results', async () => {
  const client = new ApiClient({
    server: 'https://boopmark.com', apiKey: 'key', fetchImpl: async () => response(401, { error: 'nope' }),
  });
  await assert.rejects(client.suggest('https://example.com'), (error) => {
    assert.equal(error.kind, 'auth');
    assert.equal(error.message, 'Reconnect Boopmark.');
    return true;
  });
});

test('definite create errors are surfaced separately from uncertain writes', async () => {
  const client = new ApiClient({
    server: 'https://boopmark.com', apiKey: 'key', fetchImpl: async () => response(422, { error: 'invalid fields' }),
  });
  await assert.rejects(client.create({ url: 'https://example.com', title: '', description: '', tags: [] }), (error) => {
    assert.equal(error.kind, 'definite');
    assert.equal(error.status, 422);
    return true;
  });
});

test('invalid bookmark targets fail before fetch', async () => {
  let calls = 0;
  const client = new ApiClient({
    server: 'https://boopmark.com', apiKey: 'key', fetchImpl: async () => { calls += 1; return response(201, { id: 'x' }); },
  });
  await assert.rejects(client.create({ url: 'file:///tmp/private', title: '', description: '', tags: [] }), (error) => {
    assert.equal(error.kind, 'validation');
    return true;
  });
  assert.equal(calls, 0);
});

test('create preserves the reviewed URL string instead of normalizing it', async () => {
  let request;
  const client = new ApiClient({
    server: 'https://boopmark.com', apiKey: 'key', fetchImpl: async (url, init) => {
      request = { url, init };
      return response(201, { id: 'bookmark-exact' });
    },
  });
  const reviewed = 'https://EXAMPLE.com:443/article?run=one#Fragment';
  await client.create({ url: reviewed, title: '', description: '', tags: [] });
  assert.equal(JSON.parse(request.init.body).url, reviewed);
});

test('create carries a UUID idempotency key without putting it in the URL or body', async () => {
  let request;
  const client = new ApiClient({
    server: 'https://boopmark.com', apiKey: 'key', fetchImpl: async (url, init) => {
      request = { url, init };
      return response(201, { id: 'bookmark-idempotent' });
    },
  });
  const key = 'a8098c1a-f86e-11da-bd1a-001124442be1';
  await client.create({
    url: 'https://example.com/replayed',
    title: '',
    description: '',
    tags: [],
  }, { idempotencyKey: key });
  assert.equal(request.init.headers['Idempotency-Key'], key);
  assert.equal(request.url.includes(key), false);
  assert.equal(JSON.parse(request.init.body).idempotencyKey, undefined);
});

test('create rejects a supplied invalid idempotency key before fetch', async () => {
  let calls = 0;
  const client = new ApiClient({
    server: 'https://boopmark.com', apiKey: 'key',
    fetchImpl: async () => { calls += 1; return response(201, { id: 'not-used' }); },
  });
  await assert.rejects(client.create({
    url: 'https://example.com/replayed',
    title: '',
    description: '',
    tags: [],
  }, { idempotencyKey: 'operation-not-a-uuid' }), error => {
    assert.ok(error instanceof ApiError);
    assert.equal(error.kind, 'validation');
    return true;
  });
  assert.equal(calls, 0);
});
