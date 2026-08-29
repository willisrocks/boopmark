// Read-only, fixture-scoped network observation of an already-running worker.
// No popup opening, account access, HAR, files, headers, or raw CDP logging.
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const DEFAULT_EXTENSION_ID = 'ggfienpplnccomboiahcllfpakbopane';
const API_ORIGIN = 'https://boopmark.com';
const PATHS = new Set(['/api/v1/bookmarks/suggest', '/api/v1/bookmarks']);
const emit = value => process.stdout.write(`${JSON.stringify(value)}\n`);

export function parseOptions(args, env = process.env) {
  if (args.length < 1 || args.length > 2) throw new Error('Invalid observer arguments.');
  const fixture = args[0];
  const url = new URL(fixture);
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || /[\u0000-\u0020\u007f]/.test(fixture)) throw new Error('Invalid fixture.');
  const duration = args[1] === undefined ? 60 : Number(args[1]);
  if (!Number.isInteger(duration) || duration < 1 || duration > 300) throw new Error('Invalid duration.');
  const extensionId = env.CHROME_EXTENSION_ID || DEFAULT_EXTENSION_ID;
  if (!/^[a-p]{32}$/.test(extensionId)) throw new Error('Invalid extension ID.');
  return { fixture, duration, extensionId };
}

function candidate(request) {
  if (request?.method !== 'POST') return null;
  try {
    const url = new URL(request.url);
    return url.origin === API_ORIGIN && PATHS.has(url.pathname) ? url : null;
  } catch { return null; }
}

// This projection is the only path from a request event to reported evidence.
// Deliberately never copy request headers or arbitrary query parameter values.
export function projectRequest(request, fixture) {
  const url = candidate(request);
  if (!url || typeof request.postData !== 'string' || request.postData.length > 65_536) return null;
  let payload;
  try { payload = JSON.parse(request.postData); } catch { return null; }
  if (!payload || payload.url !== fixture) return null;
  const suggest = url.searchParams.get('suggest');
  const query = !url.search ? '' : url.searchParams.size === 1 && ['true', 'false'].includes(suggest)
    ? `?suggest=${suggest}` : '[redacted unexpected query]';
  return { method: 'POST', path: url.pathname, query, status: null, failed: false, bodyCaptured: false };
}

export function projectBody(path, value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  if (path === '/api/v1/bookmarks/suggest') {
    return {
      suggestions: {
        title: typeof value.title === 'string' ? value.title.slice(0, 500) : null,
        description: typeof value.description === 'string' ? value.description.slice(0, 2_000) : null,
        tags: Array.isArray(value.tags) ? value.tags.filter(tag => typeof tag === 'string').slice(0, 30).map(tag => tag.slice(0, 100)) : [],
      },
    };
  }
  if (path === '/api/v1/bookmarks' && typeof value.id === 'string' && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value.id)) {
    return { createdId: value.id };
  }
  return null;
}

function loopbackEndpoint(value) {
  const url = new URL(value);
  if (url.protocol !== 'ws:' || url.hostname !== '127.0.0.1' || !url.port || url.username || url.password || url.search || url.hash) throw new Error('Invalid debugger endpoint.');
  return url;
}

async function findWorker(extensionId) {
  const launcher = fileURLToPath(new URL('./chrome-browser.mjs', import.meta.url));
  const result = spawnSync(process.execPath, [launcher, 'get', 'cdp-url'], {
    encoding: 'utf8', timeout: 10_000, maxBuffer: 262_144, stdio: ['ignore', 'pipe', 'pipe'],
  });
  if (result.status !== 0) throw new Error('Session unavailable.');
  const endpoints = result.stdout.split('\n').map(line => line.trim()).filter(line => line.startsWith('ws://127.0.0.1:'));
  if (endpoints.length !== 1) throw new Error('Ambiguous debugger endpoint.');
  const endpoint = loopbackEndpoint(endpoints[0]);
  if (!endpoint.pathname.startsWith('/devtools/browser/')) throw new Error('Unexpected debugger endpoint.');
  const response = await fetch(`http://${endpoint.host}/json/list`, { redirect: 'error', signal: AbortSignal.timeout(5_000) });
  if (!response.ok) throw new Error('Target listing failed.');
  const targets = await response.json();
  const workerURL = `chrome-extension://${extensionId}/worker.js`;
  const workers = Array.isArray(targets) ? targets.filter(target => target.type === 'service_worker' && target.url === workerURL) : [];
  if (workers.length !== 1) throw new Error('Exact worker must already be active.');
  const worker = workers[0];
  const socketURL = loopbackEndpoint(worker.webSocketDebuggerUrl);
  if (socketURL.origin !== endpoint.origin || socketURL.pathname !== `/devtools/page/${worker.id}`) throw new Error('Worker debugger mismatch.');
  return { socketURL: socketURL.href, workerURL, targetId: worker.id };
}

async function observe(options) {
  const target = await findWorker(options.extensionId);
  const socket = new WebSocket(target.socketURL);
  try {
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('Debugger unavailable.')), 5_000);
      socket.addEventListener('open', () => { clearTimeout(timer); resolve(); }, { once: true });
      socket.addEventListener('error', () => { clearTimeout(timer); reject(new Error('Debugger unavailable.')); }, { once: true });
    });
  } catch {
    socket.close();
    throw new Error('Debugger unavailable.');
  }
  let nextId = 0;
  const pending = new Map();
  const requests = new Map();
  let observing = false;
  let incompleteRequestBodies = false;
  let stop = () => {};
  function command(method, params = {}) {
    return new Promise((resolve, reject) => {
      const id = ++nextId;
      const timer = setTimeout(() => { pending.delete(id); reject(new Error('Debugger command unavailable.')); }, 5_000);
      pending.set(id, { resolve, reject, timer });
      socket.send(JSON.stringify({ id, method, params }));
    });
  }
  socket.addEventListener('message', async event => {
    // Raw protocol messages may contain authorization headers. They remain
    // transient in memory and are never logged, persisted, or forwarded.
    let message;
    try { message = JSON.parse(event.data); } catch { return; }
    if (message.id) {
      const waiter = pending.get(message.id);
      if (!waiter) return;
      pending.delete(message.id);
      clearTimeout(waiter.timer);
      if (message.error) waiter.reject(new Error('Debugger command unavailable.'));
      else waiter.resolve(message.result);
      return;
    }
    if (!observing) return;
    const params = message.params || {};
    if (message.method === 'Network.requestWillBeSent') {
      if (candidate(params.request) && (typeof params.request.postData !== 'string' || params.request.postData.length > 65_536)) incompleteRequestBodies = true;
      const record = projectRequest(params.request, options.fixture);
      if (record && !requests.has(params.requestId)) requests.set(params.requestId, record);
      return;
    }
    const record = requests.get(params.requestId);
    if (!record) return;
    if (message.method === 'Network.responseReceived') {
      const status = params.response?.status;
      if (Number.isInteger(status) && status >= 100 && status <= 599) record.status = status;
    } else if (message.method === 'Network.loadingFailed') {
      record.failed = true;
    } else if (message.method === 'Network.loadingFinished') {
      if (!(record.path.endsWith('/suggest') && record.status === 200) && !(record.path === '/api/v1/bookmarks' && record.status === 201)) return;
      try {
        const result = await command('Network.getResponseBody', { requestId: params.requestId });
        if (!observing || typeof result?.body !== 'string' || result.body.length > 1_048_576) return;
        const text = result.base64Encoded ? Buffer.from(result.body, 'base64').toString('utf8') : result.body;
        const fields = projectBody(record.path, JSON.parse(text));
        if (fields) { Object.assign(record, fields); record.bodyCaptured = true; }
      } catch { /* A missing body is explicit in the sanitized evidence. */ }
    }
  });
  socket.addEventListener('close', () => stop('target-closed'));
  socket.addEventListener('error', () => stop('observer-error'));

  let timer;
  let startedAt;
  const interrupted = () => stop('interrupted');
  try {
    const info = (await command('Target.getTargetInfo'))?.targetInfo;
    if (info?.targetId !== target.targetId || info?.type !== 'service_worker' || info?.url !== target.workerURL) throw new Error('Worker identity mismatch.');
    await command('Network.enable', { maxPostDataSize: 65_536 });
    const finished = new Promise(resolve => { stop = resolve; });
    process.once('SIGINT', interrupted);
    process.once('SIGTERM', interrupted);
    startedAt = new Date().toISOString();
    observing = true;
    timer = setTimeout(() => stop('duration'), options.duration * 1_000);
    emit({ type: 'observer_ready', extensionId: options.extensionId, fixture: options.fixture, durationSeconds: options.duration, startedAt, observedIntervalOnly: true });
    const reason = await finished;
    observing = false;
    const records = [...requests.values()];
    emit({
      type: 'observer_summary', startedAt, endedAt: new Date().toISOString(), stopReason: reason,
      observedIntervalOnly: true, historicalManualSave: 'unobserved', incompleteRequestBodies,
      counts: { suggest: records.filter(record => record.path.endsWith('/suggest')).length, create: records.filter(record => record.path === '/api/v1/bookmarks').length },
      requests: records,
    });
  } finally {
    observing = false;
    clearTimeout(timer);
    process.removeListener('SIGINT', interrupted);
    process.removeListener('SIGTERM', interrupted);
    for (const waiter of pending.values()) { clearTimeout(waiter.timer); waiter.reject(new Error('Observer ended.')); }
    pending.clear();
    socket.close();
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try { await observe(parseOptions(process.argv.slice(2))); }
  catch {
    // Never render child-process stderr, protocol errors, request objects, or
    // response bodies here. They may include credentials or unrelated data.
    emit({ type: 'observer_error', message: 'Observation unavailable. Check arguments, the dedicated session, and its exact active extension worker; no popup is opened automatically.' });
    process.exitCode = 1;
  }
}
