# Releasing Boopmark

The manually invoked GitHub Actions workflow named **Release** is the only supported production release path. Run it from `main` and choose exactly one semantic increment: `patch`, `minor`, or `major`.

The highest stable `vX.Y.Z` Git tag is the authoritative previous release. The workflow feeds that version and the selected increment to `npm version`, then synchronizes the result into the Rust server and CLI, Chrome manifest, iOS app, Share Extension, and XcodeGen project. It creates an immutable `release/vX.Y.Z` source branch so every build and the Railway deployment use the same commit. The release is tagged and published only after the production version endpoint reports the expected version. This tag-based baseline keeps later releases correct even though protected `main` is not mutated by a release run.

## Start a release

From GitHub, open **Actions → Release → Run workflow**, select `patch`, `minor`, or `major`, and run it on `main`. From a checkout, the equivalent is:

```sh
just release patch
```

Only one release runs at a time. A release stops before publication if tests, artifact validation, the container push, the Railway deployment, or the exact-version production check fails.

## Published assets

Each GitHub Release contains:

- Four versioned `boop` CLI binaries for Intel/Apple Silicon macOS and x86_64/ARM64 Linux.
- The exact Chrome Web Store ZIP and a separate unpack-and-load Chrome sideload ZIP.
- An unsigned iOS device IPA for re-signing and sideloading.
- An iOS Simulator `.app` ZIP.
- `INSTALL.md` with version-specific instructions.
- `SHA256SUMS` covering every downloadable artifact.

The workflow also publishes `ghcr.io/willisrocks/boopmark:X.Y.Z` and updates `ghcr.io/willisrocks/boopmark:latest`, deploys the same release commit to Railway production, and verifies both `/health` and `/version` at `https://boopmark.com`.

## Required GitHub configuration

The `production` environment uses one environment-scoped secret and two repository variables:

- Secret `RAILWAY_TOKEN` (a Railway project token scoped to production)
- Variable `RAILWAY_ENVIRONMENT_ID`
- Variable `RAILWAY_SERVICE_ID`

The workflow uses the built-in `GITHUB_TOKEN` for release branches, tags, GitHub Releases, and GHCR packages.
