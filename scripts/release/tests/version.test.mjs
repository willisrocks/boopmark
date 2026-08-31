import assert from 'node:assert/strict';
import { cp, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, resolve } from 'node:path';
import test from 'node:test';
import {
  assertReleaseVersion,
  assertVersionsMatch,
  readReleaseVersions,
  renderInstallGuide,
  syncVersion,
} from '../version-lib.mjs';

test('accepts stable semantic versions and rejects unsupported forms', () => {
  assert.equal(assertReleaseVersion('12.3.45'), '12.3.45');
  for (const invalid of ['1.2', 'v1.2.3', '01.2.3', '1.2.3-beta.1']) {
    assert.throws(() => assertReleaseVersion(invalid));
  }
});

test('requires every product version to match', () => {
  const versions = {
    npm: '2.1.0', chrome: '2.1.0', cli: '2.1.0', server: '2.1.0',
    iosProject: ['2.1.0', '2.1.0', '2.1.0', '2.1.0'],
    iosApp: '2.1.0', iosShareExtension: '2.1.0',
  };
  assert.doesNotThrow(() => assertVersionsMatch(versions, '2.1.0'));
  assert.throws(() => assertVersionsMatch({ ...versions, chrome: '2.0.0' }, '2.1.0'));
});

test('install guide names every versioned release artifact', () => {
  const guide = renderInstallGuide('3.4.5');
  for (const expected of [
    'boopmark-chrome-3.4.5.zip',
    'boopmark-chrome-sideload-3.4.5.zip',
    'boopmark-ios-simulator-3.4.5.zip',
    'boopmark-ios-unsigned-3.4.5.ipa',
    'ghcr.io/willisrocks/boopmark:3.4.5',
    'boopmark.com/version',
  ]) assert.match(guide, new RegExp(expected.replaceAll('.', '\\.')));
});

test('synchronizes Chrome, Rust, and every iOS version source', async () => {
  const repository = resolve(import.meta.dirname, '../../..');
  const fixture = await mkdtemp(resolve(tmpdir(), 'boopmark-release-version-'));
  const files = [
    'package.json', 'extensions/chrome/manifest.json', 'cli/Cargo.toml', 'server/Cargo.toml',
    'mobile/ios/project.yml', 'mobile/ios/Boopmark/Info.plist',
    'mobile/ios/BoopmarkShareExtension/Info.plist',
  ];

  try {
    for (const relative of files) {
      const target = resolve(fixture, relative);
      await mkdir(dirname(target), { recursive: true });
      await cp(resolve(repository, relative), target);
    }
    const packagePath = resolve(fixture, 'package.json');
    const packageJson = JSON.parse(await readFile(packagePath, 'utf8'));
    packageJson.version = '9.1.2';
    await writeFile(packagePath, `${JSON.stringify(packageJson, null, 2)}\n`);

    await syncVersion(fixture, '9.1.2', '912');
    const versions = await readReleaseVersions(fixture);
    assert.doesNotThrow(() => assertVersionsMatch(versions, '9.1.2'));
    assert.match(await readFile(resolve(fixture, 'mobile/ios/project.yml'), 'utf8'), /CURRENT_PROJECT_VERSION: "912"/);
  } finally {
    await rm(fixture, { recursive: true, force: true });
  }
});

test('release workflow builds immutable source and promotes latest after production', async () => {
  const repository = resolve(import.meta.dirname, '../../..');
  const dispatcher = await readFile(resolve(repository, '.github/workflows/release.yml'), 'utf8');
  const workflow = await readFile(resolve(repository, '.github/workflows/release-candidate.yml'), 'utf8');

  assert.match(dispatcher, /gh workflow run release-candidate\.yml --ref "release\/v\$VERSION" -f prepared_sha="\$SHA"/);
  assert.doesNotMatch(workflow, /needs\.prepare\.outputs\.sha/);
  assert.doesNotMatch(workflow, /ref:.*needs\./);
  assert.match(workflow, /test "\$GITHUB_SHA" = "\$PREPARED_SHA"/);
  assert.match(workflow, /git merge-base --is-ancestor "\$PARENT"/);
  assert.match(workflow, /cargo metadata --format-version=1 --locked/);
  assert.match(dispatcher, /--force-with-lease="\$RELEASE_REF:\$REMOTE_SHA"/);
  assert.match(workflow, /container:\n[\s\S]*?needs: \[metadata, quality\]/);
  assert.match(workflow, /publish:\n[\s\S]*?needs: \[metadata, deploy\]/);
  assert.match(workflow, /gh release create[\s\S]*?--draft/);
  assert.match(workflow, /gh release upload[\s\S]*?--clobber/);
  assert.doesNotMatch(workflow, /git push origin "v\$VERSION"/);
  assert.match(workflow, /promote:\n[\s\S]*?needs: \[metadata, deploy, publish\]/);
  assert.match(workflow, /docker buildx imagetools create[\s\S]*?:latest/);
  assert.match(workflow, /finalize:\n[\s\S]*?needs: \[metadata, publish, promote\]/);
  assert.match(workflow, /gh release edit[\s\S]*?--draft=false/);
});
