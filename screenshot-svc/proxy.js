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

/** Header on the proxy's refusals, so the service can tell one from a page. */
const BLOCKED_HEADER = 'x-screenshot-blocked';
const UPSTREAM_TIMEOUT_MS = 30000;
const HOP_BY_HOP = ['connection', 'keep-alive', 'proxy-connection', 'proxy-authorization', 'upgrade'];

function withoutHopByHop(headers) {
  const out = { ...headers };
  for (const name of HOP_BY_HOP) delete out[name];
  return out;
}

function refuse(res, err) {
  const blocked = err instanceof Blocked;
  if (!res.headersSent) {
    res.writeHead(blocked ? 403 : 502, { 'content-type': 'text/plain', ...(blocked && { [BLOCKED_HEADER]: '1' }) });
  }
  res.end(err.message);
}

/** Plain HTTP: the request line carries the absolute URL. */
async function forward(guard, req, res) {
  let target;
  try {
    target = new URL(req.url);
    if (target.protocol !== 'http:') throw new Blocked('the proxy forwards only http');
    const [addr] = await guard.checkUrl(target);
    const upstream = http.request({
      host: addr.address,
      family: addr.family,
      port: target.port || 80,
      method: req.method,
      path: target.pathname + target.search,
      headers: { ...withoutHopByHop(req.headers), host: target.host },
      setHost: false,
      timeout: UPSTREAM_TIMEOUT_MS,
    });
    upstream.on('timeout', () => upstream.destroy(new Error('upstream timed out')));
    upstream.on('error', err => refuse(res, err));
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
    const [addr] = await guard.resolve(target.hostname);
    const upstream = net.connect({ host: addr.address, port: Number(target.port || 443), timeout: UPSTREAM_TIMEOUT_MS });
    upstream.on('timeout', () => upstream.destroy());
    upstream.on('error', () => socket.destroy());
    socket.on('close', () => upstream.destroy());
    upstream.once('connect', () => {
      socket.write('HTTP/1.1 200 Connection Established\r\n\r\n');
      upstream.write(head);
      upstream.pipe(socket);
      socket.pipe(upstream);
    });
  } catch (err) {
    const status = err instanceof Blocked ? '403 Forbidden' : '502 Bad Gateway';
    socket.end(`HTTP/1.1 ${status}\r\n${err instanceof Blocked ? `${BLOCKED_HEADER}: 1\r\n` : ''}\r\n`);
  }
}

/** Start the proxy on a free loopback port. Resolves to `{ port, close }`. */
async function startGuardProxy(guard) {
  const server = http.createServer((req, res) => forward(guard, req, res));
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

module.exports = { BLOCKED_HEADER, startGuardProxy };
