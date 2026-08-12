# Contributing

## Toolchain

- Rust 1.97 with `rustfmt` and Clippy; the declared MSRV is 1.95.
- Node.js 22 for viewer development. The npm launcher remains compatible with Node.js 18+.
- Python 3.12 for release and schema tooling. Generated launchers support Python 3.9+.

Use the pinned `rust-toolchain.toml` and committed lockfiles. Do not add a dependency when an existing workspace library or a small local implementation is sufficient.

## Repository ownership

The Cargo workspace is split by stable responsibility:

- `api-subway-core`: public model, configuration, normalization, diagnostics;
- `api-subway-analyzers`: discovery, language indexes, framework adapters, contracts;
- `api-subway-renderer`: deterministic layout, safe SVG/HTML, browser viewer;
- `api-subway-cli`: command parsing, exit codes, and crash-safe artifact I/O.

Keep framework-specific code in its adapter. Shared parsing and traversal belong in the language index only when at least two adapters use them. The viewer's virtual backend and workspace UI remain separate modules; UI code must not own CRUD or schema-validation rules.

## Analyzer changes

Every new supported pattern needs:

1. a positive fixture that proves the endpoint and relation;
2. a negative or dynamic case that proves the analyzer does not invent a route;
3. repository-relative evidence pointing at the declaration or reachable call;
4. deterministic ordering and a diagnostic for unsupported dynamic behavior.

Do not classify an imported package as used unless a reachable handler scope calls it. Use `exact` only for a proven framework edge or resolved known-client call. Filesystem/package-role classifiers are `inferred`.

Representative multi-file applications live under `fixtures/corpus`; update `fixtures/corpus/manifest.json` when its expected surface intentionally changes. See [docs/ACCURACY.md](docs/ACCURACY.md).

## Viewer changes

Treat embedded map data and imported backend JSON as untrusted. Use the existing HTML escaping, safe property-definition helpers, bounded JSON parsing, and schema index. Do not add runtime CDN assets, framework dependencies, or network calls.

`crates/api-subway-renderer/viewer/dist/viewer.js` is committed because Rust embeds it at compile time. Rebuild it after TypeScript changes:

```bash
cd crates/api-subway-renderer/viewer
npm ci
npm run typecheck
npm run build
npm test
```

## Required checks

From the repository root:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --release --locked -p api-subway
cargo package --locked -p api-subway-core
cargo package --locked -p api-subway-analyzers --list >/dev/null
cargo package --locked -p api-subway-renderer --list >/dev/null
cargo package --locked -p api-subway --list >/dev/null
python -m unittest discover -s scripts/tests -v
python scripts/validate_schemas.py
```

Schema validation requires the pinned `jsonschema` version used in CI. Run wrapper smoke tests with `API_SUBWAY_TEST_BINARY` pointing at a release binary.

When layout, HTML, CSS, or embedded viewer output changes, regenerate the goldens:

```bash
cargo run --locked -q -p api-subway-renderer --example generate_goldens
```

When analyzer output or the demo changes, regenerate `docs/api-subway.{svg,html,json}` with the command in [README.md](README.md). Review generated diffs for absolute paths, timestamps, source snippets, and nondeterministic ordering.
