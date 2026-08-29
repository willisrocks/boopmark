import test from 'node:test';
import assert from 'node:assert/strict';
import { parseOptions, shouldAuthenticate } from '../../../scripts/e2e/chrome-library-check.mjs';

const args = ['--key-file', '/private/tmp/boopmark-extension-qa.example/boopmark-api-key', 'b1771eee-0367-4184-8855-24242824cfd0', 'https://example.org/fixture?q=exact#fragment', 'Intentional title'];
const env = { CHROME_LIBRARY_TARGET_ID: 'A'.repeat(32) };

test('library verifier requires a pinned regular target and bounded nonsecret inputs', () => {
  const options = parseOptions(args, env);
  assert.equal(options.libraryURL, `https://boopmark.com/bookmarks?search=${encodeURIComponent(args[3])}`);
  assert.throws(() => parseOptions(args, {}));
  assert.throws(() => parseOptions([...args, '/tmp/card.png'], env));
  assert.throws(() => parseOptions([...args.slice(0, 3), 'https://user:password@example.org/', args[4]], env));
  assert.throws(() => parseOptions(['secret-value', ...args.slice(1)], env));
  assert.throws(() => parseOptions([args[0], '/tmp/unscoped-key', ...args.slice(2)], env));
});

test('authorization applies only to exact filtered main-frame GET document', () => {
  const { libraryURL } = parseOptions(args, env);
  const request = { method: 'GET', url: libraryURL };
  assert.equal(shouldAuthenticate(request, 'Document', 'main', 'main', libraryURL), true);
  for (const [changedRequest, resourceType, frame] of [
    [{ ...request, method: 'POST' }, 'Document', 'main'],
    [{ ...request, url: `${libraryURL}&extra=1` }, 'Document', 'main'],
    [{ ...request, url: libraryURL.replace('boopmark.com', 'other.example') }, 'Document', 'main'],
    [request, 'XHR', 'main'], [request, 'Document', 'child'],
  ]) assert.equal(shouldAuthenticate(changedRequest, resourceType, frame, 'main', libraryURL), false);
});
