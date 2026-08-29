// Isolated, in-memory API for extension fault injection. Never forwards to production.
import http from 'node:http';

const port = Number(process.env.CHROME_FIXTURE_PORT || 4011);
const DEFAULT_SUGGESTION_DELAY_MS = 1_500;
const DEFAULT_SAVE_DELAY_MS = 2_500;
const MAX_DELAY_MS = 15_000;
const MODES = new Set(['normal', 'slow-suggest', 'suggest-error', 'empty', 'partial', 'revoked', 'save-error', 'save-unknown', 'slow-save']);
let mode = 'normal';
let events = [];
let requests = [];
let bookmarks = [];
const idempotentCreates = new Map();
const idempotencyGroups = new Map();
let nextIdempotencyGroup = 0;
let sequence = 0;
let suggestionDelayMs = DEFAULT_SUGGESTION_DELAY_MS;
let saveDelayMs = DEFAULT_SAVE_DELAY_MS;
const json = (res, status, value) => {
  res.writeHead(status, { 'Content-Type': 'application/json' });
  res.end(JSON.stringify(value));
};
const delay = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));
const has = (value, key) => Object.prototype.hasOwnProperty.call(value, key);
const validDelay = value => typeof value === 'number' && Number.isInteger(value) && value >= 0 && value <= MAX_DELAY_MS;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const validControl = command => command && typeof command === 'object' && !Array.isArray(command)
  && (!has(command, 'reset') || typeof command.reset === 'boolean')
  && (!has(command, 'mode') || MODES.has(command.mode))
  && (!has(command, 'suggestionDelayMs') || validDelay(command.suggestionDelayMs))
  && (!has(command, 'saveDelayMs') || validDelay(command.saveDelayMs));
const body = async (req) => {
  let data = '';
  for await (const chunk of req) {
    data += chunk;
    if (data.length > 32_768) throw new Error('Request too large');
  }
  return data ? JSON.parse(data) : {};
};
const reviewedFingerprint = payload => JSON.stringify({
  url: payload?.url ?? null,
  title: payload?.title ?? null,
  description: payload?.description ?? null,
  image_url: payload?.image_url ?? null,
  domain: payload?.domain ?? null,
  tags: Array.isArray(payload?.tags) ? payload.tags : payload?.tags ?? null,
});

const server = http.createServer(async (req, res) => {
  let requestRecord;
  const markStatus = status => {
    if (!requestRecord || !requestRecord.pending) return;
    delete requestRecord.pending;
    requestRecord.status = status;
  };
  const markAborted = () => {
    if (!requestRecord || !requestRecord.pending) return;
    delete requestRecord.pending;
    requestRecord.aborted = true;
  };
  const reply = (status, value) => {
    markStatus(status);
    return json(res, status, value);
  };
  try {
    const url = new URL(req.url, `http://127.0.0.1:${port}`);
    if (url.pathname === '/__control' && req.method === 'POST') {
      const command = await body(req);
      if (!validControl(command)) return json(res, 400, { error: 'Invalid fixture control' });
      if (command.reset) {
        events = [];
        requests = [];
        bookmarks = [];
        idempotentCreates.clear();
        idempotencyGroups.clear();
        nextIdempotencyGroup = 0;
      }
      if (has(command, 'mode')) mode = command.mode;
      if (has(command, 'suggestionDelayMs')) suggestionDelayMs = command.suggestionDelayMs;
      if (has(command, 'saveDelayMs')) saveDelayMs = command.saveDelayMs;
      return json(res, 200, { mode, suggestionDelayMs, saveDelayMs });
    }
    if (url.pathname === '/__state') return json(res, 200, {
      mode, suggestionDelayMs, saveDelayMs, events, requests, bookmarks,
    });
    if (url.pathname.startsWith('/article')) {
      res.writeHead(200, { 'Content-Type': 'text/html' });
      return res.end('<!doctype html><html lang="en"><head><title>Browser fallback title</title><meta name="description" content="Fixture article description"></head><body><main><h1>Boopmark extension test article</h1><p>Disposable local capture fixture. No production requests.</p><a href="/article-two">Another article</a></main></body></html>');
    }
    if (!url.pathname.startsWith('/api/')) return json(res, 404, { error: 'Not found' });
    requestRecord = { method: req.method, path: url.pathname, pending: true };
    requests.push(requestRecord);
    req.on('aborted', markAborted);
    res.on('close', () => {
      if (!res.writableEnded && !res.writableFinished) markAborted();
    });
    const requestMode = mode;
    const requestDelayMs = requestMode === 'slow-suggest' && url.pathname === '/api/v1/bookmarks/suggest' ? suggestionDelayMs
      : requestMode === 'slow-save' && url.pathname === '/api/v1/bookmarks' ? saveDelayMs : 0;
    // This fixed key is deliberately public, only for this loopback fixture.
    if (req.headers.authorization !== 'Bearer extension-fixture-key' || requestMode === 'revoked') {
      return reply(401, { error: 'Unauthorized fixture key' });
    }
    if (req.method === 'GET' && url.pathname === '/api/v1/bookmarks') {
      return reply(200, bookmarks);
    }
    const payload = await body(req);
    events.push({ method: req.method, path: url.pathname, query: url.search, payload, time: new Date().toISOString() });
    if (req.method === 'POST' && url.pathname === '/api/v1/bookmarks/suggest') {
      if (requestDelayMs) await delay(requestDelayMs);
      if (requestRecord.aborted) return;
      if (requestMode === 'suggest-error') return reply(503, { error: 'Fixture unavailable' });
      if (requestMode === 'empty') return reply(200, { title: null, description: null, tags: [] });
      if (requestMode === 'partial') return reply(200, { title: null, description: 'Partial description', tags: [] });
      return reply(200, {
        title: 'Suggested fixture title',
        description: 'Suggested fixture description',
        tags: ['testing', 'chrome'], image_url: null, domain: '127.0.0.1',
      });
    }
    if (req.method === 'POST' && url.pathname === '/api/v1/bookmarks') {
      if (requestMode === 'save-error') return reply(422, { error: 'Fixture validation rejected' });
      const rawIdempotencyKey = req.headers['idempotency-key'];
      const idempotencyKey = typeof rawIdempotencyKey === 'string' ? rawIdempotencyKey.trim() : '';
      if (rawIdempotencyKey !== undefined && !UUID.test(idempotencyKey)) {
        return reply(400, { error: 'Idempotency-Key must be a UUID' });
      }
      if (idempotencyKey) {
        if (!idempotencyGroups.has(idempotencyKey)) {
          idempotencyGroups.set(idempotencyKey, `operation-${++nextIdempotencyGroup}`);
        }
        // Expose equality across wire attempts without exposing the raw
        // operation UUID in fixture state or test artifacts.
        requestRecord.idempotencyGroup = idempotencyGroups.get(idempotencyKey);
      }
      const fingerprint = reviewedFingerprint(payload);
      const previous = idempotencyKey ? idempotentCreates.get(idempotencyKey) : null;
      if (previous) {
        if (previous.fingerprint !== fingerprint) {
          return reply(409, { error: 'Idempotency-Key was reused with a different payload' });
        }
        if (requestDelayMs) await delay(requestDelayMs);
        if (requestRecord.aborted) return;
        return reply(201, previous.record);
      }
      const record = { id: `fixture-${++sequence}`, ...payload };
      bookmarks.push(record);
      if (idempotencyKey) idempotentCreates.set(idempotencyKey, { fingerprint, record });
      if (requestMode === 'save-unknown') {
        markAborted();
        return req.socket.destroy();
      }
      if (requestDelayMs) await delay(requestDelayMs);
      if (requestRecord.aborted) return;
      return reply(201, record);
    }
    return reply(404, { error: 'Not found' });
  } catch {
    if (!res.headersSent) {
      if (requestRecord) reply(400, { error: 'Invalid fixture request' });
      else json(res, 400, { error: 'Invalid fixture request' });
    } else if (requestRecord?.pending) markAborted();
  }
});
server.listen(port, '127.0.0.1', () => console.log(`Extension fixture listening at http://127.0.0.1:${server.address().port}`));
