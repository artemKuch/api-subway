# Security policy

## Reporting a vulnerability

Use the repository's **Security → Report a vulnerability** form to open a private GitHub Security Advisory. Do not include exploit details, private source code, credentials, or unpublished findings in a public issue.

Include the affected version, platform, smallest reproducible input, expected impact, and whether the issue requires opening a generated HTML artifact or only running the CLI. Maintainers should acknowledge a complete report within five business days. Disclosure timing is coordinated after a fix and release are available.

## Supported versions

Security fixes are applied to the latest published minor release. Before the first stable release, only the newest `0.1.x` version and the default branch are supported.

## Trust boundaries

Repository source, manifests, configuration, OpenAPI documents, schema names, route paths, imported virtual-backend JSON, and generated HTML opened from an untrusted repository must all be treated as untrusted input.

The CLI:

- parses target code but never imports or executes it;
- does not install target dependencies or make network requests;
- confines import resolution and local OpenAPI files to the selected canonical root;
- does not follow symlinks outside that root;
- emits repository-relative source locations, never absolute paths or code snippets;
- escapes untrusted SVG/HTML text and neutralizes `</script>`-style JSON embedding.

The standalone HTML viewer contains no external runtime assets. Its Content Security Policy blocks network connections, objects, forms, and external scripts. The virtual backend is in-memory state, not an HTTP server, and does not run target handlers or business logic.

## Resource limits

The limits are part of the security contract, not performance hints.

| Boundary | Limit |
| --- | --- |
| TOML configuration | 1 MiB |
| One JS/TS/Python source file | 8 MiB |
| Discovered source workspace | 100,000 files and 512 MiB |
| One OpenAPI document | 16 MiB |
| OpenAPI JSON/YAML structure | 1 document, depth 64, 200,000 nodes; YAML also caps parser events at 500,000 |
| Generated `ApiMapV1` JSON | 32 MiB; collection and nested-item caps also apply |
| Imported virtual-backend JSON | 2 MB, 200 resources, 2,000 records per resource, 100,000 aggregate record nodes, depth 20, 300 keys per object |
| Browser schema simulation | 10,000 aggregate work units, validation depth 10, 2,000 array items |

OpenAPI YAML also has explicit anchor, alias, merge-key, and replay budgets. Duplicate JSON and YAML object keys are rejected. Schema graph expansion is capped per operation and produces diagnostics when a bounded subset is used.

## Supply-chain controls

Release builds run only after formatting, strict Clippy, workspace tests, viewer tests, schema validation, release-tool tests, and an OSV dependency scan. A release bundle contains deterministic archives, `SHA256SUMS`, and a CycloneDX SBOM. GitHub Actions produces Sigstore-backed provenance and SBOM attestations for published artifacts.

Consumers should verify both the checksum and repository attestation as described in [RELEASE.md](RELEASE.md).

## Security non-goals

`api-subway` does not prove that a target API is secure, that authorization is correct, or that an inferred call graph covers runtime metaprogramming. The virtual backend is an executable contract sandbox, not a secure replacement for the real application or an emulator of its side effects.
