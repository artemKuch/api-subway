# Changelog

All notable changes are documented here. The project follows Semantic Versioning.

## [Unreleased]

## [0.1.0] - 2026-07-20

### Added

- Rust CLI for deterministic SVG, standalone HTML, and `ApiMapV1` JSON generation.
- Static analyzers for Next.js App Router, Express 4/5, FastAPI, and local OpenAPI 3.0/3.1 documents.
- Reachable call/import analysis with exact and inferred dependency evidence.
- Stateful browser-local virtual backend with draggable endpoint windows and shared CRUD state.
- Schema and JSON request/response views with bounded contract validation.
- Native release packages for macOS arm64/x64, Linux arm64/x64, and Windows x64.
- Deterministic archives, checksums, CycloneDX SBOM, and GitHub artifact attestations.

### Security

- Kept analysis local and prevented target-code execution, dependency installation, network access, and symlink escape.
- Bounded source, OpenAPI, model, YAML, and virtual-backend inputs.
- Escaped untrusted SVG/HTML content and added a local-only Content Security Policy to the standalone viewer.

[Unreleased]: https://github.com/artemKuch/api-subway/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/artemKuch/api-subway/releases/tag/v0.1.0
