# Releasing Boopmark

The manually invoked GitHub Actions workflow named **Release** is the only supported production release path. Run it from `main` and choose exactly one semantic increment: `patch`, `minor`, or `major`.

The highest stable `vX.Y.Z` Git tag is the authoritative previous release. The workflow feeds that version and the selected increment to `npm version`, then synchronizes the result into the Rust server and CLI, Chrome manifest, iOS app, Share Extension, and XcodeGen project. It creates a `release/vX.Y.Z` candidate source branch, then explicitly dispatches the release-candidate workflow on that exact branch. Every build and the Railway deployment therefore execute the candidate commit as their trusted `github.sha`. A failed, unpublished candidate can be replaced safely on retry; the final release tag is immutable. The release is tagged and published only after the production version endpoint reports the expected version. This tag-based baseline keeps later releases correct even though protected `main` is not mutated by a release run.

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

The workflow publishes `ghcr.io/willisrocks/boopmark:X.Y.Z`, deploys the same release commit to Railway production, and verifies both `/health` and `/version` at `https://boopmark.com`. Only after production passes does it update `ghcr.io/willisrocks/boopmark:latest` and publish the GitHub Release.

## Retry a failed release

If a run fails before publishing its tag, correct the problem on `main` and invoke the same semantic increment again. The workflow recomputes the unpublished version and replaces only its bot-owned `release/vX.Y.Z` branch using a lease, so it cannot overwrite a concurrent update. It never replaces an existing `vX.Y.Z` tag. A versioned container from an earlier attempt may be replaced, but `latest` is not updated until the retry has passed the production verification gate.

## Required GitHub configuration

The `production` environment uses one environment-scoped secret and two repository variables:

- Secret `RAILWAY_TOKEN` (a Railway project token scoped to production)
- Variable `RAILWAY_ENVIRONMENT_ID`
- Variable `RAILWAY_SERVICE_ID`

The workflow uses the built-in `GITHUB_TOKEN` for release branches, tags, GitHub Releases, and GHCR packages.
