// Outbound URL safety for the screenshot service. Mirrors the Rust guard in
// server/src/adapters/net.rs: same address classes, ports and URL rules.

const dns = require('dns');
const net = require('net');

/** Ports a URL may name explicitly; the scheme's default is always fine. */
const ALLOWED_PORTS = [80, 443, 8080, 8443];

/** A URL or an address that the guard refuses. */
class Blocked extends Error {
  constructor(message) {
    super(message);
    this.name = 'Blocked';
  }
}

/** A URL that does not parse: bad input rather than a refusal. */
class InvalidUrl extends Blocked {
  constructor() {
    super('invalid url');
    this.name = 'InvalidUrl';
  }
}

/** A URL's hostname without IPv6 brackets: `[::1]` becomes `::1`. */
function bareHost(hostname) {
  return hostname.replace(/^\[|\]$/g, '');
}

function parseIpv4(ip) {
  return ip.split('.').map(Number);
}

/** The eight 16-bit segments of an IPv6 address, or null. */
function parseIpv6(ip) {
  ip = ip.replace(/%.*$/, ''); // zone id: fe80::1%eth0
  if (!net.isIPv6(ip)) return null;
  const dotted = ip.match(/\d+\.\d+\.\d+\.\d+$/); // ::ffff:10.0.0.1
  if (dotted) {
    const [a, b, c, d] = parseIpv4(dotted[0]);
    ip = ip.slice(0, dotted.index) + `${((a << 8) | b).toString(16)}:${((c << 8) | d).toString(16)}`;
  }
  const groups = s => (s ? s.split(':').map(h => parseInt(h, 16)) : []);
  const [head, tail] = ip.split('::').map(groups);
  if (!tail) return head;
  return [...head, ...Array(8 - head.length - tail.length).fill(0), ...tail];
}

function isPublicIpv4([a, b, c, d]) {
  return !(
    a === 10 || // private 10.0.0.0/8
    (a === 172 && (b & 0xf0) === 16) || // private 172.16.0.0/12
    (a === 192 && b === 168) || // private 192.168.0.0/16
    a === 127 || // loopback 127.0.0.0/8
    (a === 169 && b === 254) || // link-local 169.254.0.0/16
    (a === 255 && b === 255 && c === 255 && d === 255) || // broadcast
    (a === 192 && b === 0 && c === 2) || // documentation 192.0.2.0/24
    (a === 198 && b === 51 && c === 100) || // documentation 198.51.100.0/24
    (a === 203 && b === 0 && c === 113) || // documentation 203.0.113.0/24
    (a >= 224 && a < 240) || // multicast 224.0.0.0/4
    a === 0 || // "this network" 0.0.0.0/8, unspecified included
    (a === 100 && (b & 0xc0) === 64) || // shared address space 100.64.0.0/10
    (a === 192 && b === 0 && c === 0) || // IETF protocol assignments 192.0.0.0/24
    (a === 198 && (b & 0xfe) === 18) || // benchmarking 198.18.0.0/15
    a >= 240 // reserved 240.0.0.0/4
  );
}

/**
 * The IPv4 address an IPv6 address stands for, if it is one of the
 * translation or tunnelling forms that carry it in plain sight.
 */
function embeddedIpv4(s) {
  const v4 = (hi, lo) => [hi >> 8, hi & 0xff, lo >> 8, lo & 0xff];
  const zeros = n => s.slice(0, n).every(x => x === 0);
  const loopbackOrUnspecified = zeros(7) && (s[7] === 0 || s[7] === 1);
  if (loopbackOrUnspecified) return null;
  if (zeros(5) && (s[5] === 0 || s[5] === 0xffff)) return v4(s[6], s[7]); // ::ffff:a.b.c.d and ::a.b.c.d
  if (s[0] === 0x64 && s[1] === 0xff9b && s[2] === 0 && s[3] === 0 && s[4] === 0 && s[5] === 0) {
    return v4(s[6], s[7]); // NAT64 well-known prefix 64:ff9b::/96
  }
  if (s[0] === 0x2002) return v4(s[1], s[2]); // 6to4 2002:AABB:CCDD::/48
  return null;
}

function isPublicIpv6(s) {
  const [first, second, third] = s;
  return !(
    (s.slice(0, 7).every(x => x === 0) && s[7] <= 1) || // loopback ::1 and unspecified ::
    (first & 0xff00) === 0xff00 || // multicast ff00::/8
    (first & 0xfe00) === 0xfc00 || // unique-local fc00::/7, Fly's 6PN fdaa::/16 included
    (first & 0xffc0) === 0xfe80 || // link-local fe80::/10
    (first & 0xffc0) === 0xfec0 || // deprecated site-local fec0::/10
    (first === 0x64 && second === 0xff9b && third === 1) || // local-use NAT64 64:ff9b:1::/48
    (first === 0x2001 && second === 0) || // Teredo 2001::/32
    (first === 0x2001 && second === 0x0db8) || // documentation 2001:db8::/32
    (first === 0x0100 && second === 0 && third === 0 && s[3] === 0) || // discard-only 100::/64
    (first === 0x2001 && second === 0x0002 && third === 0) || // benchmarking 2001:2::/48
    (first === 0x3fff && (second & 0xf000) === 0) // documentation 3fff::/20
  );
}

/**
 * True for addresses on the public internet. Private, loopback, link-local,
 * shared (CGNAT), documentation, benchmarking, multicast, broadcast,
 * unspecified and reserved ranges are all refused. IPv6 addresses that carry
 * an IPv4 address (mapped, compatible, NAT64 64:ff9b::/96, 6to4 2002::/16)
 * are judged by that IPv4 address; Teredo 2001::/32 and the local-use NAT64
 * prefix 64:ff9b:1::/48, whose IPv4 cannot be trusted, are refused.
 * Anything that is not an IP address is refused.
 */
function isPublicIp(ip) {
  if (net.isIPv4(ip)) return isPublicIpv4(parseIpv4(ip));
  const segments = parseIpv6(ip);
  if (!segments) return false;
  const v4 = embeddedIpv4(segments);
  return v4 ? isPublicIpv4(v4) : isPublicIpv6(segments);
}

function isLoopback(ip) {
  if (net.isIPv4(ip)) return parseIpv4(ip)[0] === 127;
  const s = parseIpv6(ip);
  return !!s && s.slice(0, 7).every(x => x === 0) && s[7] === 1;
}

/** The OS resolver: every address of `host`, IPv4 and IPv6. */
async function systemLookup(host) {
  return dns.promises.lookup(host, { all: true, verbatim: true });
}

/**
 * The rules for which URLs and addresses may be fetched.
 *
 * `lookup` resolves a host name to `[{ address, family }]`. Tests pass a stub,
 * and `allowLoopback` (also any port) so they can reach a local fixture server.
 */
function createGuard({ lookup = systemLookup, allowLoopback = false } = {}) {
  const allowsIp = ip => isPublicIp(ip) || (allowLoopback && isLoopback(ip));

  /**
   * Check the parts of a URL that need no DNS lookup: scheme, credentials,
   * port, local host names and literal IP addresses. Returns the parsed URL;
   * throws `Blocked` (`InvalidUrl` when it does not parse).
   */
  function check(url) {
    if (typeof url === 'string') {
      try { url = new URL(url); } catch { throw new InvalidUrl(); }
    }
    if (url.protocol !== 'http:' && url.protocol !== 'https:') {
      throw new Blocked(`unsupported scheme: ${url.protocol.replace(/:$/, '')}`);
    }
    if (url.port && !allowLoopback && !ALLOWED_PORTS.includes(Number(url.port))) {
      throw new Blocked(`port ${url.port} is not allowed; refusing to fetch it`);
    }
    if (url.username || url.password) {
      throw new Blocked('URL contains credentials; refusing to fetch it');
    }
    const host = bareHost(url.hostname).replace(/\.$/, '').toLowerCase();
    const localName = host === 'localhost' || host.endsWith('.localhost') ||
      host.endsWith('.local') || host.endsWith('.internal');
    if (localName || (net.isIP(host) && !allowsIp(host))) {
      throw new Blocked('private/local URL; refusing to fetch it');
    }
    return url;
  }

  /**
   * Resolve `hostname` and return its addresses. Throws `Blocked` when ANY
   * address is refused: one private answer poisons the whole name.
   */
  async function resolve(hostname) {
    const host = bareHost(hostname);
    const addrs = await lookup(host);
    if (!addrs.length) throw new Error(`${host} did not resolve`);
    const bad = addrs.find(a => !allowsIp(a.address));
    if (bad) {
      throw new Blocked(`${host} resolves to a private address (${bad.address}); refusing to fetch it`);
    }
    return addrs;
  }

  /** `check` then `resolve`: the full verdict on one URL. */
  async function checkUrl(url) {
    return resolve(check(url).hostname);
  }

  return { check, resolve, checkUrl };
}

module.exports = { Blocked, InvalidUrl, bareHost, createGuard, isPublicIp };
