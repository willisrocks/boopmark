# Releasing Boopmark

The `just release` recipe on a Mac with the Depot login is the supported production release path. Choose exactly one semantic increment: `patch`, `minor`, or `major`.

The GitHub copies of `release.yml` and `release-candidate.yml` stay until a real release proves this path. A GitHub ruleset allows only the Depot Code Access app and repository admins to create a `release/v*` branch, so the GitHub copy cannot create its branch while that ruleset is active.

## How a release runs

The highest stable `vX.Y.Z` GitHub Release is the previous release. A release moves through these workflows:

1. `just release` dispatches `.depot/workflows/release.yml` on `main` and waits for it.
2. `release.yml` bumps the version, synchronizes the Rust server and CLI, the Chrome manifest, the iOS app, the Share Extension, and the XcodeGen project, and pushes a `release/vX.Y.Z` candidate branch.
3. The push from the Depot Code Access app starts `.github/workflows/release-macos.yml` on GitHub, which builds the macOS CLI binaries and the iOS packages.
4. A push from the Depot app does not start a Depot workflow. Thus `just release` dispatches `.depot/workflows/release-candidate.yml` on the candidate branch with the Depot login on the Mac.
5. `release-candidate.yml` runs the full validation, builds the Linux CLI binaries and the Chrome packages, and waits for the macOS run.
6. `release-candidate.yml` dispatches `.github/workflows/deploy.yml` on the candidate branch and fails if that run fails.
7. `deploy.yml` builds the image through the Depot project, pushes `ghcr.io/willisrocks/boopmark:X.Y.Z`, deploys that image to the Fly.io app `boopmark`, and verifies `/health` and `/version` at `https://boopmark.com`. After that check passes, it moves the `latest` tag.
8. `release-candidate.yml` publishes the GitHub Release with the Linux, Chrome, macOS, and iOS artifacts.

Only one release runs at a time. A release stops before publication if tests, artifact validation, the image push, the Fly.io deployment, or the production version check fails.

## Start a release

Run this command on the Mac that has the Depot login:

```sh
just release patch
```

The recipe prints the URL of each Depot run. It ends after it dispatches the candidate, so follow the candidate run at that URL.

## Published assets

Each GitHub Release contains:

- Four versioned `boop` CLI binaries for Intel and Apple Silicon macOS and x86_64 and ARM64 Linux
- The exact Chrome Web Store ZIP and a separate unpack-and-load Chrome sideload ZIP
- An unsigned iOS device IPA for re-signing and sideloading
- An iOS Simulator `.app` ZIP
- `INSTALL.md` with version-specific instructions
- `SHA256SUMS`, which covers every downloadable artifact

## Retry a failed release

If `just release` stops before it dispatches the candidate, for example because the Mac session ended, run the same `just release` command again. If a run fails before the tag exists, correct the problem on `main` and run the same command again.

The workflow computes the same unpublished version and replaces only its `release/vX.Y.Z` branch, with a lease, so it cannot overwrite a concurrent update. That push starts a new macOS run. If a draft release for that version exists, the workflow reuses its candidate commit instead. The workflow never replaces an existing `vX.Y.Z` tag. A versioned image from an earlier attempt can be replaced, but `latest` moves only after production verification passes.

## Required configuration

These items must exist for a release to run:

- The GitHub `production` environment, which allows only `release/v*` branches and holds the secret `FLY_API_TOKEN` (a Fly.io deploy token scoped to the `boopmark` app)
- The Depot project `boopmark` (`xs5gsx5k24`), with a GitHub trust relationship for `willisrocks/boopmark`
- The GitHub ruleset for `refs/heads/release/v*`, which allows only the Depot Code Access app (app ID 219785) and repository admins to create those branches

Depot stores no GHCR or Fly credential. `deploy.yml` pushes to GHCR with its `GITHUB_TOKEN` and authenticates to Depot with GitHub OIDC. The Depot workflows use the `GITHUB_TOKEN` that Depot provides for release branches, tags, GitHub Releases, and the dispatch of `deploy.yml`.
