# Changelog

All notable changes are documented here. The project follows Semantic Versioning.

## [Unreleased]

### Added

- Stateful browser-local virtual backend with shared CRUD revisions, draggable multi-endpoint windows, schema-driven request fields, and Schema/JSON response views.
- Request/response contract extraction from local OpenAPI, common static Zod usage, and FastAPI/Pydantic annotations.
- Exact manifest-driven accuracy corpus for representative Express 5, Next.js 16, and FastAPI applications.
- Deterministic release packaging, checksum verification, CycloneDX SBOM generation, artifact attestations, OSV scanning, and idempotent registry publishing.
- Adversarial analyzer tests, virtual CRUD property tests, input/model budgets, and schema validation.

### Changed

- JavaScript handler scopes now come from Oxc AST ranges, including multiline typed Next.js handlers and imported/barrel-forwarded handlers.
- Express mount and route detection, FastAPI module-scoped dependency resolution, and workspace-only import traversal reject unsupported dynamic cases instead of guessing.
- Standalone HTML now ships with a restrictive local-only Content Security Policy.

### Security

- Added bounded regular-file config/source/OpenAPI/output parsing, root and OpenAPI-count validation, streaming JSON node/depth limits, YAML alias/depth/replay limits, symlink-root enforcement, prototype-safe dictionaries, regex safety checks, and atomic bounded virtual-backend import.
- Native npm and Python launchers now preserve child exit status and forward termination signals; release gates verify Cargo package contents and browser import/export behavior.

## [0.1.0]

Initial public release baseline.
