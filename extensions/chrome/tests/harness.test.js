const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const root = path.resolve(__dirname, '../../..');
const launcher = path.join(root, 'scripts/e2e/chrome-browser.mjs');

for (const directory of ['test-results', 'test-results/production-profile']) {
  test(`browser harness rejects disposable profile location ${directory} before launching`, () => {
    const result = spawnSync(process.execPath, [launcher], {
      cwd: root,
      env: { ...process.env, CHROME_EXTENSION_PROFILE: path.join(root, directory) },
      encoding: 'utf8',
      timeout: 5000,
    });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Browser profiles must be outside test-results/);
    assert.equal(result.stdout, '');
  });
}
