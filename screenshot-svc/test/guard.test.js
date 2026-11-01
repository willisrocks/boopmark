const test = require('node:test');
const assert = require('node:assert');
const { Blocked, InvalidUrl, createGuard, isPublicIp } = require('../guard');
const { stubLookup } = require('./helpers');

test('public addresses are allowed', () => {
  for (const ip of ['8.8.8.8', '1.1.1.1', '93.184.216.34', '2606:4700:4700::1111', '::ffff:8.8.8.8']) {
    assert.ok(isPublicIp(ip), `${ip} is public`);
  }
});

test('non-public IPv4 ranges are refused', () => {
  for (const ip of [
    '10.1.2.3', '172.16.0.1', '172.31.255.254', '192.168.0.1', // private
    '127.0.0.1', '127.255.0.1', // loopback
    '169.254.1.1', '169.254.169.254', // link-local, cloud metadata
    '100.64.0.1', '100.127.255.255', // CGNAT
    '0.0.0.0', '0.1.2.3', // this network
    '255.255.255.255', '240.0.0.1', // broadcast, reserved
    '192.0.0.8', '192.0.2.1', '198.51.100.1', '203.0.113.1', '198.19.0.1', '224.0.0.1',
  ]) {
    assert.ok(!isPublicIp(ip), `${ip} is not public`);
  }
  for (const ip of ['172.15.255.255', '172.32.0.1', '100.63.255.255', '100.128.0.1']) {
    assert.ok(isPublicIp(ip), `${ip} is just outside a refused range`);
  }
});

test('non-public IPv6 ranges are refused', () => {
  for (const ip of [
    '::1', '::', // loopback, unspecified
    'fc00::1', 'fd12::1', 'fdaa:0:1234::2', // unique-local, Fly 6PN
    'fe80::1', 'fe80::1%eth0', 'febf::1', // link-local
    'fec0::1', '2001:db8::1', 'ff02::1',
    '2001:0:4136:e378:8000:63bf:3fff:fdd2', // Teredo
    '64:ff9b:1::808:808', // local-use NAT64: never trusted
    '100::1', '2001:2::1', '3fff::1', '3fff:fff::1', // discard-only, benchmarking, documentation
    'not-an-ip',
  ]) {
    assert.ok(!isPublicIp(ip), `${ip} is not public`);
  }
  for (const ip of ['100:0:0:1::1', '2001:2:1::1', '3fff:1000::1']) {
    assert.ok(isPublicIp(ip), `${ip} is just outside a refused range`);
  }
});

test('IPv6 forms that carry IPv4 are judged by it', () => {
  for (const ip of [
    '::ffff:127.0.0.1', '::ffff:10.0.0.1', '::ffff:a9fe:a9fe', '::127.0.0.1', // mapped, compatible
    '64:ff9b::a00:1', '64:ff9b::7f00:1', '64:ff9b::169.254.169.254', // NAT64
    '2002:a00:1::1', '2002:c0a8:101::', // 6to4 of 10.0.0.1, 192.168.1.1
  ]) {
    assert.ok(!isPublicIp(ip), `${ip} is not public`);
  }
  for (const ip of ['64:ff9b::808:808', '2002:808:808::1', '::ffff:808:808']) {
    assert.ok(isPublicIp(ip), `${ip} wraps 8.8.8.8`);
  }
});

test('check refuses bad schemes, ports, credentials and local hosts', () => {
  const guard = createGuard();
  for (const url of [
    'https://example.com/', 'http://example.com:80/', 'https://example.com:443/',
    'http://example.com:8080/', 'https://example.com:8443/', 'http://[2606:4700:4700::1111]/',
  ]) {
    assert.doesNotThrow(() => guard.check(url), url);
  }
  for (const url of [
    'not a url', 'file:///etc/passwd', 'ftp://example.com/', 'gopher://example.com/',
    'http://example.com:22/', 'http://example.com:6379/', 'https://example.com:9200/',
    'https://user:pass@example.com/', 'https://user@example.com/',
    'http://localhost/', 'http://LOCALHOST./', 'http://app.localhost/', 'http://printer.local/',
    'http://metadata.google.internal/',
    'http://127.0.0.1/', 'http://0x7f.1/', 'http://2130706433/', 'http://10.0.0.1/',
    'http://169.254.169.254/latest/meta-data/', 'http://[::1]/', 'http://[::ffff:127.0.0.1]/',
    'http://[fdaa::3]/', 'http://[64:ff9b::a00:1]/',
  ]) {
    assert.throws(() => guard.check(url), Blocked, url);
  }
  assert.throws(() => guard.check('not a url'), InvalidUrl);
  assert.strictEqual(guard.check('https://example.com/a').pathname, '/a');
});

test('resolve refuses a name when any of its addresses is private', async () => {
  const guard = createGuard({
    lookup: stubLookup({
      'public.example': ['93.184.216.34', '2606:2800:220:1::1'],
      'intranet.example': ['10.0.0.5'],
      'mixed.example': ['93.184.216.34', '192.168.1.10'],
      'mapped.example': ['::ffff:127.0.0.1'],
      'fly.example': ['fdaa:0:1::3'],
      'metadata.example': ['169.254.169.254'],
    }),
  });

  assert.strictEqual((await guard.checkUrl('https://public.example/a')).length, 2);
  await assert.rejects(guard.checkUrl('http://intranet.example/'), /private address \(10\.0\.0\.5\)/);
  for (const host of ['mixed.example', 'mapped.example', 'fly.example', 'metadata.example']) {
    await assert.rejects(guard.checkUrl(`http://${host}/`), Blocked, host);
  }
  await assert.rejects(guard.checkUrl('http://nothing.example/'), /did not resolve/);
});

test('the system resolver is the default', async () => {
  // dns.lookup answers literal addresses without a network round trip.
  await assert.rejects(createGuard().resolve('10.0.0.1'), Blocked);
  assert.deepStrictEqual(
    (await createGuard().resolve('8.8.8.8')).map(a => a.address),
    ['8.8.8.8'],
  );
});

test('allowLoopback admits loopback and any port, nothing else', async () => {
  const guard = createGuard({ allowLoopback: true, lookup: stubLookup({ 'fixture.test': ['127.0.0.1'] }) });
  assert.strictEqual((await guard.checkUrl('http://fixture.test:4123/')).length, 1);
  assert.throws(() => guard.check('http://10.0.0.1:4123/'), Blocked);
});
