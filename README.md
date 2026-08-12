# api-subway

`api-subway` turns an application API into a deterministic subway map. Every `HTTP method + path` is a station; middleware, services, integrations, and data clients are colored lines backed by source evidence.

![Example api-subway map](docs/api-subway.svg)

The committed example is generated from [`fixtures/demo`](fixtures/demo), not from hand-authored map data: Express supplies the stations and middleware, the call graph supplies service/client/data lines, and local OpenAPI supplies the executable request/response contracts.

The CLI works locally without starting the target application, installing its dependencies, or sending source code over the network. It writes a GitHub-safe SVG for README files, a standalone interactive HTML explorer, and an optional stable JSON model.

## Quick start

```bash
# npm
npx api-subway generate . --out docs/api-subway

# Python tool runners
uvx api-subway generate . --out docs/api-subway

# Rust
cargo install api-subway
api-subway generate . --out docs/api-subway
```

The default command detects supported frameworks and writes `api-subway.svg` plus `api-subway.html`:

```bash
api-subway generate [ROOT] [OPTIONS]
api-subway check [ROOT] [OPTIONS]
```

`check` performs the same analysis without writing. It exits with `1` when committed artifacts are stale and `2` for fatal errors or strict diagnostics, which makes it suitable for CI.

## Supported inputs

| Input | v0.1 coverage |
| --- | --- |
| Next.js App Router | `route.js/ts/tsx`, route groups, dynamic/catch-all segments, `proxy.ts`, and legacy `middleware.ts` constant matchers |
| Express 4/5 | direct routes, `route()` chains, handler arrays, ordered `use()`, nested router mounts, and statically resolved Zod `parse`/`safeParse` contracts |
| FastAPI | `FastAPI`, `APIRouter`, decorators, `api_route`, router prefixes, middleware, `Depends`, `Annotated`, sub-dependencies, and Pydantic request/response models |
| OpenAPI 3.0/3.1 | local JSON/YAML operations, parameters, request bodies, responses, schemas, tags, `operationId`, and security schemes |

Next Pages API, NestJS, Fastify, Hono, Django, Flask, Spring, ASP.NET, runtime tracing, and cloud analysis are intentionally outside v0.1.

## Evidence, not guesses

Every relation stores its evidence and confidence:

- `exact` means a framework declaration or an AST call to a known package client was proven. It renders as a solid line.
- `inferred` means the call itself was proven but its local boundary role (`services/`, `repositories/`, `clients/`, and similar conventions) came from a bounded classifier, or a configured rule matched. It renders as a dashed line.
- unresolved dynamic paths, mounts, and matchers become diagnostics instead of invented stations.

JavaScript/TypeScript calls are taken from Oxc AST nodes and Python calls from Tree-sitter nodes. Analysis follows imported handlers, local helpers, service/repository/client boundaries, and shared wrappers cycle-safely. An imported package creates no line until a reachable function actually calls it; evidence points to that call site and retains the local call trace that led there. Computed members that cannot be resolved emit `dynamic-dependency-call` instead of selecting a method by name.

Evidence remains available in the deterministic JSON model with repository-relative `file:line:column` locations and drives the solid/dashed visual language on the map. Endpoint windows stay focused on executable request and response contracts. Generated artifacts contain no source snippets, source payload examples, absolute paths, timestamps, or external runtime assets.

The committed multi-file accuracy corpus and known blind spots are documented in [docs/ACCURACY.md](docs/ACCURACY.md). Its exact manifest catches both missing and extra dependency relations; it is an acceptance suite, not a marketing claim about arbitrary repositories.

## Live virtual backend

The standalone HTML map includes a stateful virtual backend that runs entirely in the browser. The viewer creates one bounded JSON store from the normalized contracts when the page loads, then plans an operation for every station:

- collection `GET` reads a resource and item `GET` reads by path key;
- `POST` creates, `PUT` replaces, `PATCH` merges, and `DELETE` removes a record;
- `HEAD` and `OPTIONS` return virtual protocol results;
- non-REST action routes return a contract-shaped result and are explicitly marked `inferred`.

Clicking stations opens independent draggable request windows. Multiple windows can stay open at once, and every one uses the same current store. For example, running `PUT /orders/{id}` updates the store; an open live `GET /orders` reruns immediately and shows the changed record. Request bodies and responses both have Schema/JSON views. Schema fields expose required markers, formats, enums, and constraints, while JSON mode provides direct payload editing; changes stay synchronized in both directions. Before display, every result is projected through the schema declared for that endpoint and status: undeclared store-only fields are removed, nested objects and arrays are projected recursively, and missing required values are generated deterministically. Requests and responses are validated against the available contract, and exact/inferred behavior is visible rather than implied.

**Open backend** adds a movable JSON editor for the complete virtual store. Applying edited JSON atomically replaces the current store and resets every open endpoint window to its default request and response state. The store can also be reset, imported, or exported. Its editable format contains only the `resources` object and is documented by [`schemas/virtual-backend-v1.schema.json`](schemas/virtual-backend-v1.schema.json). Resource, record, object, recursion, and file-size budgets protect the standalone viewer from oversized input.

The JSON Schema describes the portable document shape. Runtime validation additionally requires every record to contain the resource's declared scalar `primaryKey`, requires those keys to be unique using URL-comparable string identity, and enforces a maximum nesting depth.

This is an executable contract sandbox, not target-code emulation. It never imports or runs application handlers, middleware, database clients, transforms, or business rules. CRUD behavior is exact only where the URL shape and contract prove it; ambiguous action behavior is schema-shaped and marked inferred. Static contract extraction in v0.1 supports local OpenAPI schemas, FastAPI/Pydantic annotations, and common static Zod declarations used through `parse` or `safeParse`. Dynamic transforms, refinements, computed schemas, and unproven response behavior remain diagnostics or inferred evidence.

## CLI options

| Option | Meaning |
| --- | --- |
| `--framework auto\|next\|express\|fastapi` | Select an adapter; repeat to combine adapters |
| `--format svg\|html\|json` | Select output formats; repeat as needed |
| `--openapi FILE` | Merge a local OpenAPI document; repeat as needed |
| `--config FILE` | Use a TOML configuration file |
| `--theme auto\|paper\|midnight` | Select the artifact theme |
| `--strict` | Return exit code `2` for warning/error diagnostics |

Configuration is loaded from `ROOT/.api-subway.toml` by default. See [`.api-subway.example.toml`](.api-subway.example.toml) for all v1 fields and custom dependency rules.

## Visual grammar

- Stations are ordered by canonical path and then `GET, POST, PUT, PATCH, DELETE, OPTIONS, HEAD, ANY`.
- Districts use the first static URL segment.
- A shared dependency becomes a line when it reaches at least two stations or is pinned.
- README SVG limits the map to the most connected lines; HTML retains every line with search, method/kind filters, pan, zoom, theme switching, draggable live endpoint windows, the shared JSON backend, and contract validation.
- Interchange rings mark endpoints connected to multiple visible lines. Exact and inferred meaning is encoded with line style as well as color.

## Configuration

```toml
version = 1
frameworks = ["auto"]
openapi = ["openapi.yaml"]

[output]
base = "docs/api-subway"
formats = ["svg", "html", "json"]
theme = "auto"

[scan]
exclude = ["generated/**", "vendor/**"]

[map]
group_by = "path-prefix"
max_lines = 12
min_line_stations = 2
include_routes = ["/api/**"]
exclude_routes = ["/api/internal/**"]

[[dependency]]
name = "Billing service"
kind = "service"
packages = ["@acme/billing"]
path_globs = ["src/billing/**"]
pin = true
```

Unknown configuration keys, unsupported framework names, malformed globs, and invalid configuration versions fail fast.

## Architecture

The repository is a Cargo workspace with clear ownership boundaries:

```text
crates/
  api-subway-core/       stable ApiMapV1, config, paths, diagnostics
  api-subway-analyzers/  Oxc + Tree-sitter indexes and framework adapters
  api-subway-renderer/   layout, safe SVG, standalone HTML, TS viewer
  api-subway-cli/        commands, deterministic I/O, exit codes
packages/
  npm/                   native platform package selector
  python/                native platform wheel launcher
```

JavaScript and TypeScript use Oxc parsing, semantic analysis, and Node/TS resolution. Python uses `tree-sitter-python` plus a workspace-only import resolver. Handler traversal is cycle-safe and remains inside the selected root.

Regenerate the real interactive example after analyzer or viewer changes with:

```bash
cargo run -p api-subway -- generate fixtures/demo \
  --format svg --format html --format json \
  --out "$PWD/docs/api-subway" --theme midnight
```

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release -p api-subway

cd crates/api-subway-renderer/viewer
npm ci
npm run typecheck
npm run build
npm test
```

Golden JSON/SVG/HTML artifacts cover 10, 40, and 100 stations. Framework fixtures cover positive and deliberately unresolved cases. Release tags build native artifacts, npm platform packages, and Python platform wheels for macOS arm64/x64, Linux arm64/x64, and Windows x64.

The published macOS binaries require macOS 11 or newer. Linux artifacts target glibc/manylinux 2.35; musl/Alpine is not supported in v0.1.

Contributor workflow and analyzer evidence rules are in [CONTRIBUTING.md](CONTRIBUTING.md). The tag-to-registry process and consumer verification commands are in [RELEASE.md](RELEASE.md).

Performance measurements use a release build and stay outside ordinary CI time gates. The default benchmark creates 1,000 route files with approximately 100,000 lines; the larger budget can be reproduced with `--files 10000 --lines-per-file 100`:

```bash
python scripts/benchmark.py --binary target/release/api-subway
```

The script reports min/median/max wall time and throughput as JSON so benchmark results can be recorded without making normal CI sensitive to runner variance. See [BENCHMARKS.md](BENCHMARKS.md) for the current baseline.

## Security model

- Target code is parsed, never imported or executed.
- Dependency installation and network access are not part of analysis.
- Directory traversal does not follow symlinks, and OpenAPI inputs resolving outside the root are rejected.
- Source and OpenAPI JSON/YAML inputs have explicit size and structure budgets.
- The virtual backend is in-memory browser state only; JSON import is validated and bounded before it atomically replaces the current store.
- SVG excludes scripts, external resources, and `foreignObject`; all untrusted names are escaped.
- Standalone HTML uses an explicit local-only Content Security Policy; release bundles include checksums, a CycloneDX SBOM, and GitHub artifact attestations.

See [SECURITY.md](SECURITY.md) for trust boundaries, exact resource limits, vulnerability reporting, and supply-chain controls.

## License

MIT. See [LICENSE](LICENSE).
