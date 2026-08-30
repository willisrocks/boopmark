const test = require('node:test');
const assert = require('node:assert/strict');
const { pathToFileURL } = require('node:url');
const path = require('node:path');
const observer = import(pathToFileURL(path.resolve(__dirname, '../../../scripts/e2e/chrome-observe.mjs')).href);
const fixture = 'https://example.com/article?run=observer#section';

test('observer accepts only bounded duration and valid exact extension identity', async () => {
  const { parseOptions } = await observer;
  assert.equal(parseOptions([fixture], {}).duration, 60);
  assert.equal(parseOptions([fixture, '300'], {}).duration, 300);
  for (const value of ['0', '301', '1.5', 'NaN']) assert.throws(() => parseOptions([fixture, value], {}));
  assert.throws(() => parseOptions(['https://user:secret@example.com'], {}));
  assert.throws(() => parseOptions([fixture], { CHROME_EXTENSION_ID: 'other' }));
});

test('observer request projection excludes headers, payload fields, GETs, and other fixtures', async () => {
  const { projectRequest } = await observer;
  const request = {
    method: 'POST', url: 'https://boopmark.com/api/v1/bookmarks?suggest=true',
    headers: { Authorization: 'Bearer must-not-leak', 'Idempotency-Key': '123e4567-e89b-42d3-a456-426614174000' },
    postData: JSON.stringify({ url: fixture, title: 'Not a reported request field', secret: 'must-not-leak' }),
  };
  const result = projectRequest(request, fixture);
  assert.equal(result.query, '?suggest=true');
  assert.deepEqual(result.idempotencyKey, { present: true, validUuid: true });
  assert.equal(JSON.stringify(result).includes('must-not-leak'), false);
  assert.equal(JSON.stringify(result).includes('reported request'), false);
  assert.equal(projectRequest({ ...request, method: 'GET' }, fixture), null);
  assert.equal(projectRequest(request, 'https://example.com/other'), null);
  assert.equal(projectRequest({ ...request, url: 'https://elsewhere.example/api/v1/bookmarks' }, fixture), null);
  const unexpected = projectRequest({ ...request, url: 'https://boopmark.com/api/v1/bookmarks?token=must-not-leak' }, fixture);
  assert.equal(unexpected.query, '[redacted unexpected query]');
  const missing = projectRequest({ ...request, headers: {} }, fixture);
  assert.deepEqual(missing.idempotencyKey, { present: false, validUuid: false });
  const suggestion = projectRequest({ ...request, url: 'https://boopmark.com/api/v1/bookmarks/suggest' }, fixture);
  assert.equal(suggestion.idempotencyKey, null);
});

test('observer response projection reports only suggestion fields or a created UUID', async () => {
  const { projectBody } = await observer;
  const result = projectBody('/api/v1/bookmarks/suggest', {
    title: 'Title', description: 'Description', tags: ['one', { key: 'must-not-leak' }],
    headers: { Authorization: 'must-not-leak' }, debug: 'must-not-leak',
  });
  assert.deepEqual(result, { suggestions: { title: 'Title', description: 'Description', tags: ['one'] } });
  const id = 'b1771eee-0367-4184-8855-24242824cfd0';
  assert.deepEqual(projectBody('/api/v1/bookmarks', { id, title: 'Not reported', api_key: 'must-not-leak' }), { createdId: id });
  assert.equal(projectBody('/api/v1/bookmarks', { id: 'must-not-leak' }), null);
});
