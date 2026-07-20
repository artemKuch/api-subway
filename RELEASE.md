# Release process

`api-subway` releases are built from annotated `vMAJOR.MINOR.PATCH` tags that already exist on `main`. GitHub Actions verifies the source, builds every supported native target, produces attestations, and creates the GitHub Release.

Registry publication is a separate guarded step. It runs only when the repository variable `PUBLISH_REGISTRIES` is exactly `true`.

## One-time repository setup

- Create a protected GitHub environment named `release` with required reviewers.
- Protect `main` and `v*` tags; allow release tags only from commits reachable from `main`.
- Reserve `api-subway` on npm, PyPI, and crates.io plus the `@api-subway/*` npm scope.
- Configure `NPM_TOKEN` and `CARGO_REGISTRY_TOKEN` in the `release` environment.
- Configure a PyPI Trusted Publisher for this repository, workflow, and environment.

GitHub Release creation, provenance, and SBOM attestations use `GITHUB_TOKEN` and OIDC. No signing key is stored in the repository.

## Prepare a release

1. Update `CHANGELOG.md` and every version field together.
2. Rebuild the viewer and regenerate committed golden/demo artifacts.
3. Run every check in [CONTRIBUTING.md](CONTRIBUTING.md).
4. Validate repository versions:

   ```bash
   RELEASE_VERSION=0.1.0 PYTHONPATH=scripts python3 -c \
     'import os; from release_artifacts import validate_repository_version; validate_repository_version(os.environ["RELEASE_VERSION"])'
   ```

5. Commit the release state to `main`.
6. Create and push an annotated tag:

   ```bash
   git tag -a v0.1.0 -m "Release v0.1.0"
   git push origin main v0.1.0
   ```

Stable `X.Y.Z` versions only. Metadata mismatches, malformed tags, or tags not reachable from `main` fail before native builds start.

## Automated release gates

The release workflow:

1. runs Rust, viewer, schema, release-tool, and OSV checks;
2. builds and smoke-tests five native targets;
3. packages native archives, npm platform packages, and Python wheels;
4. verifies the complete artifact manifest and checksums;
5. generates a CycloneDX SBOM and Sigstore-backed attestations;
6. creates the GitHub Release;
7. publishes registries only when explicitly enabled.

Linux packages target glibc/manylinux 2.35; musl/Alpine is not supported in v0.1. macOS binaries require macOS 11 or newer. Windows packages target x64 MSVC.

## Verify a downloaded release

From the directory containing the release bundle:

```bash
shasum -a 256 -c SHA256SUMS
gh attestation verify api-subway-0.1.0-x86_64-unknown-linux-gnu.tar.gz \
  --repo artemKuch/api-subway
```

The same command verifies npm archives and wheels. The SBOM is named `api-subway-VERSION.cdx.json`.

## Recovery

Never overwrite or reuse a published version. Registry publication is idempotent, so a partially completed publish can rerun for the same immutable tag. For a defective release, keep its assets available for auditability, document the issue, yank or deprecate affected registry packages where supported, and publish a patch release.
