import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

export const VERSION_PATTERN = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;

export function assertReleaseVersion(version) {
  if (!VERSION_PATTERN.test(version)) {
    throw new Error(`release version must be stable SemVer (X.Y.Z), got: ${version}`);
  }
  return version;
}

function replaceAllChecked(source, pattern, replacement, expected, label) {
  const matches = source.match(pattern) ?? [];
  if (matches.length !== expected) {
    throw new Error(`${label}: expected ${expected} version fields, found ${matches.length}`);
  }
  return source.replace(pattern, replacement);
}

async function updateJson(path, update) {
  const value = JSON.parse(await readFile(path, 'utf8'));
  update(value);
  await writeFile(path, `${JSON.stringify(value, null, 2)}\n`);
}

export async function syncVersion(repository, version, buildNumber) {
  assertReleaseVersion(version);
  if (!/^[1-9]\d*$/.test(String(buildNumber))) {
    throw new Error(`iOS build number must be a positive integer, got: ${buildNumber}`);
  }

  const packageJson = JSON.parse(await readFile(join(repository, 'package.json'), 'utf8'));
  if (packageJson.version !== version) {
    throw new Error(`npm version must run before synchronization (package.json is ${packageJson.version})`);
  }

  await updateJson(join(repository, 'extensions/chrome/manifest.json'), (manifest) => {
    manifest.version = version;
  });

  for (const relative of ['cli/Cargo.toml', 'server/Cargo.toml']) {
    const path = join(repository, relative);
    const source = await readFile(path, 'utf8');
    await writeFile(
      path,
      replaceAllChecked(source, /^version = "[^"]+"$/m, `version = "${version}"`, 1, relative),
    );
  }

  const projectPath = join(repository, 'mobile/ios/project.yml');
  let project = await readFile(projectPath, 'utf8');
  project = replaceAllChecked(
    project,
    /^        MARKETING_VERSION: "[^"]+"$/gm,
    `        MARKETING_VERSION: "${version}"`,
    4,
    'mobile/ios/project.yml MARKETING_VERSION',
  );
  project = replaceAllChecked(
    project,
    /^        CURRENT_PROJECT_VERSION: "[^"]+"$/gm,
    `        CURRENT_PROJECT_VERSION: "${buildNumber}"`,
    4,
    'mobile/ios/project.yml CURRENT_PROJECT_VERSION',
  );
  await writeFile(projectPath, project);

  for (const relative of [
    'mobile/ios/Boopmark/Info.plist',
    'mobile/ios/BoopmarkShareExtension/Info.plist',
  ]) {
    const path = join(repository, relative);
    let plist = await readFile(path, 'utf8');
    plist = replaceAllChecked(
      plist,
      /(<key>CFBundleShortVersionString<\/key>\s*<string>)[^<]+(<\/string>)/g,
      `$1${version}$2`,
      1,
      `${relative} CFBundleShortVersionString`,
    );
    plist = replaceAllChecked(
      plist,
      /(<key>CFBundleVersion<\/key>\s*<string>)[^<]+(<\/string>)/g,
      `$1${buildNumber}$2`,
      1,
      `${relative} CFBundleVersion`,
    );
    await writeFile(path, plist);
  }
}

export async function readReleaseVersions(repository) {
  const packageJson = JSON.parse(await readFile(join(repository, 'package.json'), 'utf8'));
  const manifest = JSON.parse(
    await readFile(join(repository, 'extensions/chrome/manifest.json'), 'utf8'),
  );
  const cli = await readFile(join(repository, 'cli/Cargo.toml'), 'utf8');
  const server = await readFile(join(repository, 'server/Cargo.toml'), 'utf8');
  const project = await readFile(join(repository, 'mobile/ios/project.yml'), 'utf8');
  const appPlist = await readFile(join(repository, 'mobile/ios/Boopmark/Info.plist'), 'utf8');
  const extensionPlist = await readFile(
    join(repository, 'mobile/ios/BoopmarkShareExtension/Info.plist'),
    'utf8',
  );

  const cargoVersion = (source) => source.match(/^version = "([^"]+)"$/m)?.[1];
  const marketingVersions = [...project.matchAll(/MARKETING_VERSION: "([^"]+)"/g)].map(
    (match) => match[1],
  );
  const plistVersion = (source) =>
    source.match(/<key>CFBundleShortVersionString<\/key>\s*<string>([^<]+)<\/string>/)?.[1];

  return {
    npm: packageJson.version,
    chrome: manifest.version,
    cli: cargoVersion(cli),
    server: cargoVersion(server),
    iosProject: marketingVersions,
    iosApp: plistVersion(appPlist),
    iosShareExtension: plistVersion(extensionPlist),
  };
}

export function assertVersionsMatch(versions, expected) {
  assertReleaseVersion(expected);
  const values = [
    versions.npm,
    versions.chrome,
    versions.cli,
    versions.server,
    ...versions.iosProject,
    versions.iosApp,
    versions.iosShareExtension,
  ];
  const mismatches = values.filter((version) => version !== expected);
  if (versions.iosProject.length !== 4 || mismatches.length > 0) {
    throw new Error(`release versions are not synchronized to ${expected}: ${JSON.stringify(versions)}`);
  }
}

export function renderInstallGuide(version, repositorySlug = 'willisrocks/boopmark') {
  assertReleaseVersion(version);
  return `# Install Boopmark v${version}

These files were built from the source commit tagged \`v${version}\`. Verify downloads with \`SHA256SUMS\` before installing them.

## Chrome extension

### Sideload in Chrome

1. Download and unzip \`boopmark-chrome-sideload-${version}.zip\`.
2. Open \`chrome://extensions\` and enable **Developer mode**.
3. Select **Load unpacked** and choose the extracted \`boopmark-chrome-unpacked-${version}\` folder.
4. Pin Boopmark, open it, set the server to \`https://boopmark.com\`, enter a Boopmark API key, and select **Connect**.

The \`boopmark-chrome-${version}.zip\` file is the exact package intended for the Chrome Web Store developer dashboard; Chrome's **Load unpacked** flow uses the sideload archive instead.

## iOS app

### Simulator

1. Download and unzip \`boopmark-ios-simulator-${version}.zip\` on a Mac with Xcode.
2. Boot an iPhone simulator.
3. Run \`xcrun simctl install booted Boopmark.app\`, then launch Boopmark from the simulator.

### Physical device sideload

The \`boopmark-ios-unsigned-${version}.ipa\` contains the app and Share Extension but is intentionally unsigned. Re-sign it with your own Apple Developer identity and matching App Group/Keychain entitlements using a sideloading tool such as Sideloadly or AltStore, then install it on the device. Free Apple IDs may require periodic re-signing. Never install an IPA you have not verified against \`SHA256SUMS\`.

## Command-line client

Download the binary matching your platform, make it executable, and place it on your \`PATH\`. For example:

\`\`\`sh
chmod +x boop-aarch64-apple-darwin
sudo mv boop-aarch64-apple-darwin /usr/local/bin/boop
boop --version
\`\`\`

Linux builds are named \`boop-x86_64-unknown-linux-gnu\` and \`boop-aarch64-unknown-linux-gnu\`; macOS builds use \`apple-darwin\`.

## Container and hosted web app

The release container is \`ghcr.io/${repositorySlug}:${version}\` and the hosted app is deployed to [boopmark.com](https://boopmark.com). Confirm the running production release at [boopmark.com/version](https://boopmark.com/version).
`;
}
