const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const root = path.resolve(__dirname, '..');
const manifest = JSON.parse(fs.readFileSync(path.join(root, 'manifest.json')));

test('MV3 package has only user-invoked tab access and optional API origins', () => {
  assert.equal(manifest.manifest_version, 3);
  assert.deepEqual(manifest.permissions, ['activeTab', 'storage']);
  assert.equal(manifest.host_permissions, undefined);
  assert.equal(manifest.content_scripts, undefined);
  assert.equal(manifest.externally_connectable, undefined);
  assert.equal(manifest.web_accessible_resources, undefined);
  assert.deepEqual(manifest.optional_host_permissions, ['https://*/*', 'http://localhost/*', 'http://127.0.0.1/*', 'http://[::1]/*']);
  assert.match(manifest.content_security_policy.extension_pages, /script-src 'self'/);
  assert.doesNotMatch(manifest.content_security_policy.extension_pages, /unsafe-eval|unsafe-inline|https:/);
  assert.equal(manifest.background.type, 'module');
  for (const file of [manifest.action.default_popup, manifest.background.service_worker]) {
    assert.equal(path.basename(file), file);
    assert.equal(fs.existsSync(path.join(root, file)), true);
  }
});

test('packaged Chrome icons are correctly sized PNGs of the existing brand', () => {
  for (const [size, file] of Object.entries(manifest.icons)) {
    const png = fs.readFileSync(path.join(root, file));
    assert.equal(png.subarray(1, 4).toString(), 'PNG');
    assert.equal(png.readUInt32BE(16), Number(size));
    assert.equal(png.readUInt32BE(20), Number(size));
  }
  assert.equal(fs.readFileSync(path.join(root, 'logo.svg'), 'utf8'), fs.readFileSync(path.join(root, '../../static/boopmark-logo.svg'), 'utf8'));
});
