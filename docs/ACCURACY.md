# Analyzer accuracy and limitations

`api-subway` optimizes for trustworthy omissions over visually impressive guesses. A station or line must have a framework declaration, a reachable call/import trace, an OpenAPI operation, or an explicit configuration rule. Unsupported dynamic behavior is reported as a diagnostic.

## What the accuracy corpus proves

`fixtures/corpus/manifest.json` is a machine-readable acceptance contract for three representative multi-file applications:

- Express 5 with ordered JSON/session middleware, a mounted router, service/repository boundaries, `pg`, and Twilio;
- Next.js 16 App Router with Proxy matchers, multiline typed handlers, a dynamic segment, services, Prisma, and OpenAI;
- FastAPI with `src/` layout, router prefixing, `Depends`, service imports, SQLAlchemy, and Stripe.

The corpus currently fixes 11 `method + path` stations and every expected dependency-to-station mapping. Tests require zero diagnostics for these supported forms and reject both missing and extra relations. This is 100% conformance to the committed corpus; it is not a claim of 100% precision or recall across arbitrary public repositories.

Smaller fixtures separately cover route groups, catch-all paths, legacy middleware, Express route chains/mount order/RegExp paths, FastAPI `Annotated` and sub-dependencies, barrel re-exports, duplicate helper names in different modules, OpenAPI merge behavior, and deliberately unresolved dynamic cases.

## Confidence semantics

| Confidence | Required proof |
| --- | --- |
| `exact` | Framework middleware/dependency declaration, OpenAPI contract/security declaration, or reachable Oxc/Tree-sitter call to a catalogued client |
| `inferred` | Reachable local call plus a bounded filesystem/package-role classifier, or an explicit configured dependency rule |

An import alone is not usage. Call-graph traversal starts at each handler, follows local/imported helpers cycle-safely, and keeps evidence scoped to that endpoint. Evidence includes repository-relative source positions and the traversed call/import chain.

## Supported static forms

### Next.js App Router

- `app` and `src/app` route handlers in JS/JSX/TS/TSX;
- named HTTP exports, dynamic/catch-all/optional catch-all segments, and route groups;
- `proxy.ts` and legacy `middleware.ts` with constant matchers;
- local helpers, imported handlers, default/named imports, and barrel re-exports.

### Express 4/5

- proven Express app/router bindings;
- `app|router.METHOD`, `route(path).METHOD`, handler arrays, ordered `use`, and nested static mounts;
- literal paths, route parameters, and supported RegExp paths;
- common static Zod `parse`/`safeParse` request/response flows.

### FastAPI

- `FastAPI`/`APIRouter`, method decorators, `api_route`, and nested `include_router` prefixes;
- application/router/route dependencies, `Depends`, `Annotated`, and sub-dependencies;
- project-root and `src/` package layouts with workspace-only absolute/relative import resolution;
- common Pydantic request and response annotations.

### OpenAPI

- local OpenAPI 3.0/3.1 JSON or YAML;
- paths, operations, parameters, request bodies, responses, local `$ref`, schemas, tags, `operationId`, and security requirements;
- code/spec operation merge; code remains the dependency-evidence source.

## Known blind spots

- computed route paths, environment-derived mount prefixes, runtime router registration, decorators produced by factories, monkey-patching, and dependency injection resolved only at runtime;
- external OpenAPI `$ref`, executable schema transforms/refinements, arbitrary Zod/Pydantic metaprogramming, and response values inferred only from handler control flow;
- framework plugins outside the v0.1 support table;
- database type inference when only a generic client is proven. The line says `Prisma`, `SQLAlchemy`, or `Postgres client`, not an unproven database product.

The virtual backend uses the extracted contract and REST-shaped URL semantics. It does not execute handler code, middleware, authorization, database queries, external calls, or business rules. Non-REST actions are explicitly inferred.

## Adding evidence for a missed pattern

Reduce the repository to the smallest multi-file fixture that preserves the behavior. Add the exact expected station and `kind:name` relation mapping, plus a negative neighbor showing what must not match. A fix is complete only when its evidence points to the correct declaration/call and all artifacts remain deterministic.
