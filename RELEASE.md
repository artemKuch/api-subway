# Release process

Releases are tag-driven and intentionally separate build, attestation, GitHub publication, and registry publication.

## Repository setup

Configure a protected GitHub environment named `release` with required reviewers. Registry publication is disabled unless the repository variable `PUBLISH_REGISTRIES` is exactly `true`.

Before the first public tag, reserve the `api-subway` name on npm, PyPI, and crates.io and create the npm organization that owns `@api-subway/*`. Recheck availability immediately before reservation; a previous availability check is not an ownership guarantee.

Protect the `main` branch and `v*` tags, allow release tags only from commits already reachable from `main`, and enable immutable GitHub Releases for the repository. Both GitHub Release creation and registry publication use the protected `release` environment; required reviewers therefore guard every privileged publication step.

The environment needs:

- `NPM_TOKEN` with publish access to `api-subway` and `@api-subway/*`;
- `CARGO_REGISTRY_TOKEN` with publish access to the four workspace crates;
- a PyPI Trusted Publisher matching `.github/workflows/release.yml`, the repository, and the `release` environment.

GitHub's `GITHUB_TOKEN` and OIDC identity provide release creation and artifact attestations. No signing key is stored in the repository.

## Prepare a release

1. Update `CHANGELOG.md` and all version fields together: Cargo workspace, npm launcher, five npm platform manifests, Python project/package, and viewer package.
2. Build the committed viewer and regenerate goldens and the README demo.
3. Run the full checks in [CONTRIBUTING.md](CONTRIBUTING.md), including Cargo package verification for all four publishable crates.
4. Validate metadata with:

   ```bash
   RELEASE_VERSION=0.1.0 PYTHONPATH=scripts python -c \
     'import os; from release_artifacts import validate_repository_version; validate_repository_version(os.environ["RELEASE_VERSION"])'
   ```

5. Commit the release state to `main`, create an annotated `vMAJOR.MINOR.PATCH` tag on that commit, and push the tag.

Only stable `MAJOR.MINOR.PATCH` versions are publishable. Prerelease/build suffixes, invalid tags, metadata mismatches, and tags not reachable from `main` fail before native builds start.

## Automated gates

The release workflow:

1. repeats Rust, viewer, schema, release-tool, and OSV checks;
2. builds native binaries on Linux x64/arm64, macOS x64/arm64, and Windows x64;
3. smoke-tests npm and Python launchers on every native runner;
4. creates deterministic native archives, npm platform packages, and platform wheels;
5. assembles the npm launcher, CycloneDX 1.5 SBOM, and `SHA256SUMS`;
6. rejects missing, extra, empty, unsafe, or checksum-mismatched artifacts;
7. generates GitHub provenance and SBOM attestations;
8. creates the GitHub Release;
9. optionally publishes platform packages before launchers/crates through the protected environment.

Publication scripts are idempotent and poll registries after each dependency package becomes visible. This makes an interrupted release safe to resume with the same immutable version.

Linux packages target glibc (`manylinux_2_35`) and do not support musl/Alpine in v0.1. macOS binaries target macOS 11 or newer. Windows packages target x64 MSVC. npm platform packages are marked `preferUnplugged` so package managers execute the native binary from a real filesystem path.

## Verify a downloaded release

From the directory containing the release bundle:

```bash
shasum -a 256 -c SHA256SUMS
gh attestation verify api-subway-0.1.0-x86_64-unknown-linux-gnu.tar.gz \
  --repo api-subway/api-subway
```

The same attestation command can verify npm archives and wheels. The SBOM is `api-subway-VERSION.cdx.json`.

## Recovery

Never reuse or overwrite a published version. If a registry publish partially succeeds, fix the workflow and rerun the same tag so idempotent steps publish only missing artifacts. For a defective release, document the issue, yank/deprecate affected registry versions where supported, and publish a new patch version. GitHub release assets and attestations should remain available for auditability.
