const test = require('node:test');
const assert = require('node:assert/strict');
const Core = require('../core.js');

test('server URLs require HTTPS except loopback development origins', () => {
  assert.equal(Core.normalizeServer('https://boopmark.com/'), 'https://boopmark.com');
  assert.equal(Core.normalizeServer('http://127.0.0.1:4011/'), 'http://127.0.0.1:4011');
  assert.equal(Core.normalizeServer('http://localhost/'), 'http://localhost');
  assert.equal(Core.normalizeServer('http://example.com/'), null);
  assert.equal(Core.normalizeServer('https://boopmark.com/settings'), null);
  assert.equal(Core.normalizeServer('https://user:secret@boopmark.com/'), null);
  assert.equal(Core.normalizeServer('https://boopmark.com/?key=value'), null);
});

test('bookmark URL validation preserves query and fragment while rejecting credentials', () => {
  const url = 'https://example.com/article?run=one#review';
  assert.equal(Core.validBookmarkURL(url), true);
  assert.equal(Core.canonicalBookmarkURL(url), url);
  assert.equal(Core.validBookmarkURL('file:///tmp/bookmark'), false);
  assert.equal(Core.validBookmarkURL('javascript:alert(1)'), false);
  assert.equal(Core.validBookmarkURL('https://user:secret@example.com/'), false);
  assert.equal(Core.validBookmarkURL('https://example.com/\n'), false);
});

test('tag parsing trims and removes empty entries', () => {
  assert.deepEqual(Core.parseTags(' design, , software ,, tools '), ['design', 'software', 'tools']);
  assert.equal(Core.formatTags([' design ', '', 'software']), 'design, software');
});

test('dirty fields and intentional clears survive suggestion merges', () => {
  let draft = Core.createDraft({
    id: 'draft-one', server: 'https://boopmark.com', connectionEpoch: 'one',
    tabId: 2, url: 'https://example.com/a', title: 'Browser title'
  });
  const begun = Core.beginSuggestion(draft);
  draft = begun.draft;
  draft = Core.updateField(draft, 'description', '');
  const merged = Core.applySuggestion(draft, {
    title: 'Server title', description: 'Server description', tags: ['one']
  }, begun.token);
  assert.equal(merged.draft.title, 'Server title');
  assert.equal(merged.draft.description, '');
  assert.equal(merged.draft.tags, 'one');
  assert.equal(merged.applied, 2);
});

test('URL generations reject stale A responses, including A to B to A', () => {
  let draft = Core.createDraft({
    id: 'draft-two', server: 'https://boopmark.com', connectionEpoch: 'one',
    tabId: 3, url: 'https://example.com/a', title: 'A'
  });
  const a1 = Core.beginSuggestion(draft);
  draft = a1.draft;
  draft = Core.changeURL(draft, 'https://example.com/b');
  const b = Core.beginSuggestion(draft);
  draft = b.draft;
  draft = Core.changeURL(draft, 'https://example.com/a');
  const a2 = Core.beginSuggestion(draft);
  draft = a2.draft;
  assert.equal(Core.isCurrentRequest(draft, a1.token), false);
  assert.equal(Core.isCurrentRequest(draft, b.token), false);
  assert.equal(Core.isCurrentRequest(draft, a2.token), true);
});

test('URL changes clear generated values but preserve authored values', () => {
  let draft = Core.createDraft({
    id: 'draft-three', server: 'https://boopmark.com', connectionEpoch: 'one',
    tabId: 4, url: 'https://example.com/a', title: 'Browser title'
  });
  const begun = Core.beginSuggestion(draft);
  draft = Core.applySuggestion(begun.draft, {
    title: 'Generated title', description: 'Generated description', tags: ['generated']
  }, begun.token).draft;
  // A provisional browser title is also cleared for a genuinely different
  // URL, then restored only when the user returns to the original capture.
  draft = Core.changeURL(draft, 'https://example.com/b');
  assert.equal(draft.title, '');
  draft = Core.changeURL(draft, 'https://example.com/a');
  assert.equal(draft.title, 'Browser title');
  const nextSuggestion = Core.beginSuggestion(draft);
  draft = Core.applySuggestion(nextSuggestion.draft, {
    title: 'Generated title', description: 'Generated description', tags: ['generated']
  }, nextSuggestion.token).draft;
  draft = Core.updateField(draft, 'description', 'Authored description');
  draft = Core.changeURL(draft, 'https://example.com/b');
  assert.equal(draft.title, '');
  assert.equal(draft.description, 'Authored description');
  assert.equal(draft.tags, '');
});

test('save payload includes explicit strings and a parsed tags array', () => {
  assert.deepEqual(Core.serializeCreatePayload({
    url: 'https://example.com/?x=1#fragment', title: '', description: '', tags: 'one, , two'
  }), {
    url: 'https://example.com/?x=1#fragment', title: '', description: '', tags: ['one', 'two']
  });
});

test('pending operation recovers as unknown and never asks for replay', () => {
  const marker = Core.operationMarker({ url: 'https://example.com', title: '', description: '', tags: [] }, {
    id: 'operation-one', server: 'https://boopmark.com', connectionEpoch: 'one', draftId: 'draft-one', submittedAt: 10
  });
  assert.equal(marker.state, 'pending');
  const recovered = Core.recoverOperation(marker, false);
  assert.equal(recovered.state, 'unknown');
  assert.equal(Core.recoverOperation(marker, true).state, 'pending');
});

test('operation markers always carry a UUID id for server idempotency', () => {
  const marker = Core.operationMarker({ url: 'https://example.com', title: '', description: '', tags: [] }, {
    id: 'not-a-uuid', server: 'https://boopmark.com', connectionEpoch: 'one', draftId: 'draft-one',
  });
  assert.match(marker.id, /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i);
});

test('draft normalization keeps the URL dirty bit and safe error text', () => {
  const draft = Core.normalizeDraft({
    id: 'draft-four', server: 'https://boopmark.com', url: 'https://example.com',
    dirty: { url: true, title: true }, error: { kind: 'server', message: 'Metadata failed' },
  });
  assert.equal(draft.dirty.url, true);
  assert.equal(draft.dirty.title, true);
  assert.deepEqual(draft.error, { kind: 'server', message: 'Metadata failed' });
});

test('page identity matches the server rules for the capture URL check', () => {
  assert.equal(Core.samePage('https://example.com/a', 'http://example.com/a'), true, 'http and https are the same page');
  assert.equal(Core.samePage('https://example.com/a/', 'https://example.com/a'), true, 'one trailing slash is ignored');
  assert.equal(Core.samePage('https://example.com/a#fragment', 'https://example.com/a'), true, 'fragments are ignored');
  assert.equal(Core.samePage('https://example.com/a?utm_source=news&b=2&a=1', 'https://example.com/a?a=1&b=2'), true, 'tracking params dropped, order ignored');
  assert.equal(Core.samePage('https://example.com/a?source=medium', 'https://example.com/a'), false, '?source= is kept');
  assert.equal(Core.samePage('https://example.com/A', 'https://example.com/a'), false, 'paths are case sensitive');
  assert.equal(Core.samePage('https://example.com:8443/a', 'https://example.com/a'), false, 'a non-default port differs');
  assert.equal(Core.samePage('https://user:secret@example.com/a', 'https://example.com/a'), false, 'userinfo never matches');
  assert.equal(Core.samePage('https://other.example/a', 'https://example.com/a'), false);
  assert.equal(Core.samePage('https://example.com/a', 'chrome://extensions'), false);
  assert.equal(Core.samePage('', 'https://example.com/a'), false);
});

test('page capture is limited to web pages and the server byte cap', () => {
  assert.equal(Core.PAGE_CAPTURE_MAX_BYTES, 5242880);
  assert.equal(Core.PAGE_CAPTURE_STORAGE_MAX_BASE64, 4 * 1024 * 1024);
  assert.equal(Core.capturableURL('https://example.com/article'), true);
  assert.equal(Core.capturableURL('http://localhost:4000/article'), true);
  assert.equal(Core.capturableURL('chrome://extensions'), false);
  assert.equal(Core.capturableURL('file:///tmp/page.html'), false);
  assert.equal(Core.capturableURL('about:blank'), false);
  assert.equal(Core.capturableURL('data:text/html,<p>x</p>'), false);
  assert.equal(Core.capturableURL('https://user:secret@example.com/'), false);
});

test('a capture outcome is recorded on a successful operation only', () => {
  const marker = Core.operationMarker(
    { url: 'https://example.com/a', title: '', description: '', tags: [] },
    { server: 'https://boopmark.com', connectionEpoch: 'one', draftId: 'draft' }
  );
  const success = Core.transitionOperation(marker, {
    state: 'success', bookmarkId: 'bookmark', capture: { state: 'captured', text: 'article text captured' }
  });
  assert.deepEqual(success.capture, { state: 'captured', text: 'article text captured' });
  const failed = Core.transitionOperation(marker, {
    state: 'error', error: 'nope', capture: { state: 'captured', text: 'article text captured' }
  });
  assert.equal(failed.capture, undefined);
});
