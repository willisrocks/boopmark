import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
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
  assert.match(workflow, /git rev-list -n 1 "v\$VERSION"[\s\S]*?gh release view[\s\S]*?\.isDraft/);
  assert.match(workflow, /git merge-base --is-ancestor "\$PARENT"/);
  assert.match(workflow, /cargo metadata --format-version=1 --locked/);
  assert.match(dispatcher, /--force-with-lease="\$RELEASE_REF:\$REMOTE_SHA"/);
  assert.match(dispatcher, /gh release list --exclude-drafts --exclude-pre-releases/);
  assert.match(dispatcher, /reuse_sha=\$REUSE_SHA/);
  assert.match(dispatcher, /if test -n "\$REUSE_SHA"; then[\s\S]*?SHA="\$REUSE_SHA"/);
  assert.match(workflow, /container:\n[\s\S]*?needs: \[metadata, quality\]/);
  assert.match(workflow, /publish:\n[\s\S]*?needs: \[metadata, deploy\]/);
  assert.match(workflow, /pattern: cli-\*/);
  assert.match(workflow, /pattern: chrome-\*/);
  assert.match(workflow, /pattern: ios-\*/);
  assert.doesNotMatch(workflow, /with: \{path: release, merge-multiple: true\}/);
  assert.match(workflow, /gh release create[\s\S]*?--draft/);
  assert.match(workflow, /gh release upload[\s\S]*?--clobber/);
  assert.doesNotMatch(workflow, /git push origin "v\$VERSION"/);
  assert.match(workflow, /promote:\n[\s\S]*?needs: \[metadata, deploy, publish\]/);
  assert.match(workflow, /docker buildx imagetools create[\s\S]*?:latest/);
  assert.match(workflow, /finalize:\n[\s\S]*?needs: \[metadata, publish, promote\]/);
  assert.match(workflow, /gh release edit[\s\S]*?--draft=false/);

  // A deploy that cannot start must report the Fly machine state and app logs and
  // fail on positive evidence, without shortening the healthy ten-minute window.
  const deployJob = workflow.slice(workflow.indexOf('\n  deploy:\n'), workflow.indexOf('\n  publish:\n'));
  assert.match(deployJob, /id: deploy/);
  assert.match(deployJob, /FLY_API_TOKEN/);
  assert.match(deployJob, /flyctl status --app "\$app" --json/);
  assert.match(deployJob, /flyctl machine list --app "\$app" --json/);
  assert.match(deployJob, /flyctl logs --app "\$app" --no-tail/);
  assert.match(deployJob, /grace_seconds=60/);
  assert.match(deployJob, /started_at=\$\(date -u \+%s\)/);
  assert.match(deployJob, /request\.exit_event\.exit_code/);
  // Fly updates machines in place, so events still hold the previous release's exit:
  // only the newest event, newer than this step, may be treated as breakage.
  assert.match(deployJob, /sort_by\(epoch \/\/ 0\) \| last/);
  assert.match(deployJob, /test "\$\{event_type:-\}" = exit/);
  assert.match(deployJob, /-ge "\$started_at"/);
  assert.doesNotMatch(deployJob, /select\(\.type == "exit"\)\] \| last/);
  assert.match(deployJob, /failed\|crashed/);
  assert.match(deployJob, /last app log lines/);
  assert.match(deployJob, /within 10 minutes/);
  assert.doesNotMatch(deployJob, /deadline_seconds/);
  assert.doesNotMatch(deployJob, /select\(\.state \| IN/);
  assert.doesNotMatch(deployJob, /^\s*flyctl (secrets|config)\b/m);
  // Fly machine events carry epoch milliseconds, not ISO-8601.
  assert.match(deployJob, /if type == "number"/);
  assert.match(deployJob, /then \(if \. > 1e11 then \(\. \/ 1000 \| floor\) else \. end\)/);
});

test('deploy diagnostics judge breakage by the newest Fly machine event', async (t) => {
  if (spawnSync('jq', ['--version'], { encoding: 'utf8' }).error?.code === 'ENOENT') {
    return t.skip('jq is not installed');
  }

  const repository = resolve(import.meta.dirname, '../../..');
  const workflow = await readFile(resolve(repository, '.github/workflows/release-candidate.yml'), 'utf8');
  const program = workflow.match(/(def epoch:[\s\S]*?)' 2>\/dev\/null/)?.[1];
  assert.ok(program, 'the analysis jq program is in the workflow');

  const image = 'ghcr.io/willisrocks/boopmark:1.2.3';
  const stepStart = 1793000000;
  const analyze = (events) => {
    const machines = [{ id: 'm1', state: 'started', config: { image }, instance_id: 'i1', events }];
    const result = spawnSync('jq', ['-r', '--arg', 'tag', ':1.2.3', program], {
      input: JSON.stringify(machines),
      encoding: 'utf8',
    });
    assert.equal(result.status, 0, result.stderr);
    const [type, exitCode, newestAt] = result.stdout.trim().split('\t').slice(3);
    return { type, exitCode: Number(exitCode), newestAt: Number(newestAt) };
  };

  // Fly stops the previous release in place, so its exit stays in `.events`: an exit
  // of 143 older than the step start must not count, even after a newer start.
  const stale = analyze([
    { type: 'exit', timestamp: (stepStart - 600) * 1000, request: { exit_event: { exit_code: 143 } } },
    { type: 'start', timestamp: (stepStart + 5) * 1000 },
  ]);
  assert.deepEqual(stale, { type: 'start', exitCode: 0, newestAt: stepStart + 5 });

  // Events arrive newest-first, so array order must not be trusted.
  const died = analyze([
    { type: 'exit', timestamp: (stepStart + 30) * 1000, request: { exit_event: { exit_code: 1 } } },
    { type: 'start', timestamp: stepStart * 1000 },
  ]);
  assert.deepEqual(died, { type: 'exit', exitCode: 1, newestAt: stepStart + 30 });
  assert.ok(died.newestAt >= stepStart);
});
