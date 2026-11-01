// The guard applied to requests, with a stub resolver and no browser: the
// page.route handler, and the proxy that every Chromium request goes through.
const test = require('node:test');
const assert = require('node:assert');
const http = require('http');
const net = require('net');
const { createGuard } = require('../guard');
const { guardPage } = require('../index');
const { BLOCKED_HEADER, startGuardProxy } = require('../proxy');

function stubLookup(table) {
  return async host => (table[host] || []).map(address => ({ address, family: net.isIPv6(address) ? 6 : 4 }));
}

const quiet = { warn() {} };

test('the route handler aborts refused requests and continues the rest', async () => {
  const guard = createGuard({
    lookup: stubLookup({
      'public.example': ['93.184.216.34'],
      'intranet.example': ['10.0.0.5'],
      'fly.example': ['93.184.216.34', 'fdaa:0:1::3'],
    }),
  });
  let handler;
  await guardPage({ route: async (pattern, fn) => { assert.strictEqual(pattern, '**/*'); handler = fn; } }, guard, quiet);

  async function verdict(url) {
    let outcome;
    await handler({
      request: () => ({ url: () => url }),
      abort: async reason => { outcome = `abort:${reason}`; },
      continue: async () => { outcome = 'continue'; },
    });
    return outcome;
  }

  assert.strictEqual(await verdict('https://public.example/style.css'), 'continue');
  assert.strictEqual(await verdict('http://public.example:8080/'), 'continue');
  for (const url of [
    'http://intranet.example/',
    'http://fly.example/',
    'http://169.254.169.254/latest/meta-data/',
    'http://[::ffff:10.0.0.1]/',
    'http://public.example:6379/',
    'https://user:pw@public.example/',
    'http://unknown.example/',
  ]) {
    assert.strictEqual(await verdict(url), 'abort:blockedbyclient', url);
  }
});

/** A fixture server on loopback that records the paths it serves. */
async function fixture() {
  const hits = [];
  const server = http.createServer((req, res) => {
    hits.push(req.url);
    res.end(`hello ${req.headers.host}`);
  });
  await new Promise(r => server.listen(0, '127.0.0.1', r));
  return { hits, port: server.address().port, close: () => new Promise(r => server.close(r)) };
}

function viaProxy(proxyPort, url) {
  return new Promise((resolve, reject) => {
    http.get({ host: '127.0.0.1', port: proxyPort, path: url }, res => {
      let body = '';
      res.on('data', c => { body += c; });
      res.on('end', () => resolve({ status: res.statusCode, headers: res.headers, body }));
    }).on('error', reject);
  });
}

function connectVia(proxyPort, target) {
  return new Promise((resolve, reject) => {
    http.request({ host: '127.0.0.1', port: proxyPort, method: 'CONNECT', path: target })
      .on('connect', (res, socket) => { socket.destroy(); resolve(res.statusCode); })
      .on('error', reject)
      .end();
  });
}

test('the proxy forwards public hosts to the checked address and refuses the rest', async t => {
  const site = await fixture();
  t.after(site.close);
  const guard = createGuard({
    allowLoopback: true,
    // intranet.test would reach the fixture if the guard let one private
    // answer through.
    lookup: stubLookup({ 'public.test': ['127.0.0.1'], 'intranet.test': ['127.0.0.1', '10.0.0.5'] }),
  });
  const proxy = await startGuardProxy(guard);
  t.after(proxy.close);

  const ok = await viaProxy(proxy.port, `http://public.test:${site.port}/page?q=1`);
  assert.strictEqual(ok.status, 200);
  assert.strictEqual(ok.body, `hello public.test:${site.port}`);

  for (const url of [
    `http://intranet.test:${site.port}/secret`,
    `http://10.0.0.1:${site.port}/secret`,
    `http://user:pw@public.test:${site.port}/secret`,
  ]) {
    const refused = await viaProxy(proxy.port, url);
    assert.strictEqual(refused.status, 403, url);
    assert.strictEqual(refused.headers[BLOCKED_HEADER], '1', url);
  }
  assert.strictEqual((await viaProxy(proxy.port, `http://nothing.test:${site.port}/`)).status, 502);

  assert.strictEqual(await connectVia(proxy.port, `public.test:${site.port}`), 200);
  assert.strictEqual(await connectVia(proxy.port, `intranet.test:${site.port}`), 403);
  assert.strictEqual(await connectVia(proxy.port, `[fdaa::3]:443`), 403);

  assert.deepStrictEqual(site.hits, ['/page?q=1']);
});

test('without allowLoopback the proxy refuses loopback and non-web ports', async t => {
  const site = await fixture();
  t.after(site.close);
  const proxy = await startGuardProxy(createGuard({ lookup: stubLookup({ 'public.test': ['93.184.216.34'] }) }));
  t.after(proxy.close);

  assert.strictEqual((await viaProxy(proxy.port, `http://127.0.0.1:${site.port}/`)).status, 403);
  assert.strictEqual((await viaProxy(proxy.port, `http://localhost/`)).status, 403);
  assert.strictEqual(await connectVia(proxy.port, 'public.test:22'), 403);
  assert.strictEqual(await connectVia(proxy.port, `127.0.0.1:${site.port}`), 403);
  assert.deepStrictEqual(site.hits, []);
});
