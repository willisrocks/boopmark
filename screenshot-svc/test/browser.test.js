// End to end through real Chromium, with a stub resolver. Skipped when
// Playwright or a Chromium it can launch is missing; set CHROMIUM_PATH to
// use a system Chromium (e.g. on NixOS).
const test = require('node:test');
const assert = require('node:assert');
const http = require('http');
const { createGuard } = require('../guard');
const { createServer, launchBrowser } = require('../index');
const { listen, stubLookup } = require('./helpers');

const guard = createGuard({
  allowLoopback: true,
  // Both names point at the fixture; intranet.test also has a private
  // answer, so the fixture sees a request for it only if the guard fails.
  lookup: stubLookup({
    'public.test': ['127.0.0.1'],
    'intranet.test': ['127.0.0.1', '10.0.0.5'],
    'dead.test': ['127.0.0.1'],
  }),
});

function post(port, url) {
  return new Promise((resolve, reject) => {
    const req = http.request({ host: '127.0.0.1', port, method: 'POST', path: '/screenshot' }, res => {
      const chunks = [];
      res.on('data', c => chunks.push(c));
      res.on('end', () => resolve({ status: res.statusCode, type: res.headers['content-type'], body: Buffer.concat(chunks) }));
    });
    req.on('error', reject);
    req.end(JSON.stringify({ url }));
  });
}

test('Chromium never reaches a private host by subresource or redirect', async t => {
  let launched;
  try {
    launched = await launchBrowser(guard, { executablePath: process.env.CHROMIUM_PATH });
  } catch (err) {
    t.skip(`no launchable Chromium: ${err.message.split('\n')[0]}`);
    return;
  }
  t.after(launched.close);

  const hits = [];
  const site = http.createServer((req, res) => {
    hits.push(`${req.headers.host.split(':')[0]}${req.url}`);
    if (req.url === '/redirect') {
      res.writeHead(302, { location: `http://intranet.test:${sitePort}/secret` });
      res.end();
    } else if (req.url === '/redirect-dead') {
      res.writeHead(302, { location: `http://dead.test:${deadPort}/` });
      res.end();
    } else if (req.url === '/page') {
      res.writeHead(200, { 'content-type': 'text/html' });
      res.end(`<img src="http://public.test:${sitePort}/ok.png">
        <img src="http://intranet.test:${sitePort}/secret.png">
        <img src="http://public.test:${sitePort}/redirect">`);
    } else {
      res.end();
    }
  });
  const sitePort = await listen(site);
  // A port nothing listens on: the proxy's fetch fails with a 502 of its own.
  const closed = http.createServer();
  const deadPort = await listen(closed);
  await new Promise(r => closed.close(r));
  t.after(() => new Promise(r => site.close(r)));
  const service = createServer({ browser: launched.browser, guard, settleMs: 200 });
  const servicePort = await listen(service);
  t.after(() => new Promise(r => service.close(r)));

  const shot = await post(servicePort, `http://public.test:${sitePort}/page`);
  assert.strictEqual(shot.status, 200, shot.body.toString());
  assert.strictEqual(shot.type, 'image/jpeg');
  assert.ok(hits.includes('public.test/ok.png'), hits.join(' '));

  const redirected = await post(servicePort, `http://public.test:${sitePort}/redirect`);
  assert.strictEqual(redirected.status, 403, redirected.body.toString());

  // The proxy's error page must fail the capture, not become the thumbnail.
  const dead = await post(servicePort, `http://public.test:${sitePort}/redirect-dead`);
  assert.strictEqual(dead.status, 500, dead.body.toString());
  assert.match(dead.body.toString(), /fetch failed/);

  const direct = await post(servicePort, `http://intranet.test:${sitePort}/page`);
  assert.strictEqual(direct.status, 403);

  assert.deepStrictEqual(hits.filter(h => h.startsWith('intranet.test')), []);
});
