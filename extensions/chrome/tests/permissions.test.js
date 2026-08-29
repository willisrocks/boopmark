const test = require('node:test');
const assert = require('node:assert/strict');
const { permissionPattern } = require('../core.js');

test('API host grants use explicit effective ports, including default ports and IPv6', () => {
  assert.equal(permissionPattern('https://boopmark.com'), 'https://boopmark.com:443/*');
  assert.equal(permissionPattern('https://boopmark.com:443/'), 'https://boopmark.com:443/*');
  assert.equal(permissionPattern('http://localhost'), 'http://localhost:80/*');
  assert.equal(permissionPattern('http://127.0.0.1:4011'), 'http://127.0.0.1:4011/*');
  assert.equal(permissionPattern('https://selfhost.example:8443'), 'https://selfhost.example:8443/*');
  assert.equal(permissionPattern('http://[::1]:4011'), 'http://[::1]:4011/*');
  assert.throws(() => permissionPattern('https://user:secret@example.com'));
});
