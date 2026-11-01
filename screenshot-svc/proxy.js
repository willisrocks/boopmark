// A forward proxy on loopback that Chromium sends all of its traffic through.
//
// page.route() does not see redirect hops (Chromium follows a 3xx without
// asking the route handler again) nor service-worker requests, and the
// browser would resolve host names itself after the route check, leaving a
// DNS-rebinding window. The proxy closes all three: every request and every
// hop arrives here, the host name is checked and resolved here, and the
// connection goes to exactly the address that was checked.

const http = require('http');
const net = require('net');
const { Blocked } = require('./guard');

/**
 * Header on every response the proxy makes up itself, `blocked` or `failed`,
 * so the service never screenshots one as if it were the page.
 */
const PROXY_ERROR_HEADER = 'x-screenshot-proxy-error';
const UPSTREAM_TIMEOUT_MS = 30000;
/** Open client connections, tunnels included, beyond which new ones are dropped. */
const MAX_CONNECTIONS = 256;
const HOP_BY_HOP = ['connection', 'keep-alive', 'proxy-connection', 'proxy-authorization', 'upgrade'];

function withoutHopByHop(headers) {
  const out = { ...headers };
  for (const name of [...HOP_BY_HOP, PROXY_ERROR_HEADER]) delete out[name];
  return out;
}

/**
 * Connect options that reach `hostname` only at its already-checked
 * `addrs`, trying each in turn (Happy Eyeballs), so a dead IPv6 answer falls
 * back to IPv4 without a second, unchecked lookup.
 */
function pinnedTo(hostname, addrs) {
  return {
    host: hostname.replace(/^\[|\]$/g, ''),
    autoSelectFamily: true,
    lookup: (_host, opts, cb) => (opts.all ? cb(null, addrs) : cb(null, addrs[0].address, addrs[0].family)),
  };
}

function refuse(res, err) {
  const blocked = err instanceof Blocked;
  if (res.headersSent || res.destroyed) {
    res.destroy();
    return;
  }
  res.writeHead(blocked ? 403 : 502, {
    'content-type': 'text/plain',
    [PROXY_ERROR_HEADER]: blocked ? 'blocked' : 'failed',
  });
  res.end(err.message);
}

/** Plain HTTP: the request line carries the absolute URL. */
async function forward(guard, req, res) {
  let target;
  try {
    target = new URL(req.url);
    if (target.protocol !== 'http:') throw new Blocked('the proxy forwards only http');
    const addrs = await guard.checkUrl(target);
    const upstream = http.request({
      ...pinnedTo(target.hostname, addrs),
      port: target.port || 80,
      method: req.method,
      path: target.pathname + target.search,
      headers: { ...withoutHopByHop(req.headers), host: target.host },
      setHost: false,
      timeout: UPSTREAM_TIMEOUT_MS,
    });
    upstream.on('timeout', () => upstream.destroy(new Error('upstream timed out')));
    upstream.on('error', err => refuse(res, err));
    // Chromium gave up on the request: stop fetching it.
    res.on('close', () => upstream.destroy());
    upstream.on('response', up => {
      res.writeHead(up.statusCode, withoutHopByHop(up.headers));
      up.pipe(res);
    });
    req.pipe(upstream);
  } catch (err) {
    if (!(err instanceof Blocked) && !target) err = new Blocked('invalid url');
    refuse(res, err);
  }
}

/** HTTPS and WebSockets: CONNECT host:port, then a raw tunnel. */
async function tunnel(guard, req, socket, head) {
  socket.on('error', () => {});
  try {
    const target = new URL(`https://${req.url}`);
    if (target.pathname !== '/' || target.search || target.username) throw new Blocked('invalid CONNECT target');
    guard.checkHostPort(target.hostname, target.port);
    const addrs = await guard.resolve(target.hostname);
    const upstream = net.connect({ ...pinnedTo(target.hostname, addrs), port: Number(target.port || 443), timeout: UPSTREAM_TIMEOUT_MS });
    upstream.on('timeout', () => upstream.destroy());
    upstream.on('error', () => {});
    upstream.on('close', () => socket.destroy());
    socket.on('close', () => upstream.destroy());
    upstream.once('connect', () => {
      socket.write('HTTP/1.1 200 Connection Established\r\n\r\n');
      upstream.write(head);
      upstream.pipe(socket);
      socket.pipe(upstream);
    });
  } catch (err) {
    const status = err instanceof Blocked ? '403 Forbidden' : '502 Bad Gateway';
    const kind = err instanceof Blocked ? 'blocked' : 'failed';
    socket.end(`HTTP/1.1 ${status}\r\n${PROXY_ERROR_HEADER}: ${kind}\r\n\r\n`);
  }
}

/** Start the proxy on a free loopback port. Resolves to `{ port, close }`. */
async function startGuardProxy(guard, { maxConnections = MAX_CONNECTIONS } = {}) {
  const server = http.createServer((req, res) => forward(guard, req, res));
  server.maxConnections = maxConnections;
  // closeAllConnections() no longer sees a socket once it is a tunnel.
  const tunnels = new Set();
  server.on('connect', (req, socket, head) => {
    tunnels.add(socket);
    socket.on('close', () => tunnels.delete(socket));
    tunnel(guard, req, socket, head);
  });
  // A plain ws:// upgrade arrives as a request with an Upgrade header; refuse
  // it rather than tunnel an unchecked protocol.
  server.on('upgrade', (req, socket) => socket.destroy());
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  return {
    port: server.address().port,
    close: () => new Promise(resolve => {
      for (const socket of tunnels) socket.destroy();
      server.closeAllConnections();
      server.close(() => resolve());
    }),
  };
}

module.exports = { PROXY_ERROR_HEADER, startGuardProxy };
