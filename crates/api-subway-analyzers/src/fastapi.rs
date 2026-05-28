use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
    sync::LazyLock,
};

use api_subway_core::{
    Confidence, DependencyKind, Diagnostic, DiagnosticSeverity, Endpoint, Evidence, EvidenceKind,
    SourceRef, canonical_endpoint_id, district_for_path, normalize_route_path,
};
use regex::Regex;

use crate::{
    discovery::{AdapterOutput, ExplicitDependency, RouteRecord},
    javascript::SourceLocator,
    python::{PythonFile, PythonIndex},
};

const MAX_ROUTER_CONTEXTS: usize = 10_000;
const MAX_DEPENDENCIES_PER_ROUTE: usize = 1_000;
const MAX_CALL_BODY_BYTES: usize = 1024 * 1024;
const MAX_CALL_SCAN_WORK: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
struct RouterDefinition {
    name: String,
    prefix: String,
    dependencies: BTreeSet<DependencyRef>,
    is_app: bool,
}

#[derive(Debug, Clone)]
struct RawRoute {
    router: String,
    methods: Vec<String>,
    path: String,
    offset: usize,
    dependencies: BTreeSet<DependencyRef>,
    entry_symbol: Option<String>,
}

#[derive(Debug, Clone)]
struct RouterInclude {
    parent: String,
    child_file: PathBuf,
    child_hint: String,
    prefix: String,
    dependencies: BTreeSet<DependencyRef>,
}

#[derive(Debug)]
struct ParsedFile<'a> {
    file: &'a PythonFile,
    locator: SourceLocator,
    routers: Vec<RouterDefinition>,
    routes: Vec<RawRoute>,
    includes: Vec<RouterInclude>,
    middleware: BTreeMap<String, BTreeSet<DependencyRef>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
struct RouterContext {
    prefix: String,
    dependencies: BTreeSet<DependencyRef>,
}

struct CallScanBudget {
    remaining: usize,
    exhausted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DependencyNode {
    file: PathBuf,
    symbol: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DependencyRef {
    name: String,
    node: Option<DependencyNode>,
}

pub(crate) fn analyze(_root: &Path, index: &PythonIndex) -> AdapterOutput {
    let function_dependencies = collect_function_dependencies(index);
    let mut output = AdapterOutput::empty();
    let mut parsed = BTreeMap::<PathBuf, ParsedFile<'_>>::new();
    for file in index.files() {
        if !(file.source.contains("FastAPI(")
            || file.source.contains("APIRouter(")
            || file.source.contains("include_router("))
        {
            continue;
        }
        if !file.parse_ok {
            output.diagnostics.push(Diagnostic {
                code: "fastapi-parse".to_owned(),
                severity: DiagnosticSeverity::Warning,
                message: format!(
                    "Tree-sitter reported syntax errors in {}; decorators were recovered conservatively",
                    file.relative
                ),
                source: Some(SourceRef {
                    file: file.relative.clone(),
                    line: 1,
                    column: 1,
                }),
            });
        }
        let parsed_file = parse_file(file, &mut output.diagnostics);
        if !parsed_file.routers.is_empty() || !parsed_file.routes.is_empty() {
            parsed.insert(file.path.clone(), parsed_file);
        }
    }

    let mut contexts = BTreeMap::<(PathBuf, String), BTreeSet<RouterContext>>::new();
    let mut context_count = 0_usize;
    let mut context_budget_reported = false;
    'application_contexts: for (path, file) in &parsed {
        for router in file.routers.iter().filter(|router| router.is_app) {
            if context_count >= MAX_ROUTER_CONTEXTS {
                context_budget_reported = true;
                break 'application_contexts;
            }
            let mut dependencies = router.dependencies.clone();
            dependencies.extend(
                file.middleware
                    .get(&router.name)
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
            let inserted = contexts
                .entry((path.clone(), router.name.clone()))
                .or_default()
                .insert(RouterContext {
                    prefix: router.prefix.clone(),
                    dependencies,
                });
            if inserted {
                context_count += 1;
            }
        }
    }
    if contexts.is_empty() {
        'router_contexts: for (path, file) in &parsed {
            for router in file.routers.iter().filter(|router| !router.is_app) {
                if context_count >= MAX_ROUTER_CONTEXTS {
                    context_budget_reported = true;
                    break 'router_contexts;
                }
                let inserted = contexts
                    .entry((path.clone(), router.name.clone()))
                    .or_default()
                    .insert(RouterContext {
                        prefix: router.prefix.clone(),
                        dependencies: router.dependencies.clone(),
                    });
                if inserted {
                    context_count += 1;
                }
            }
        }
    }
    if context_budget_reported {
        output.diagnostics.push(Diagnostic {
            code: "fastapi-context-budget".to_owned(),
            severity: DiagnosticSeverity::Warning,
            message: format!(
                "Stopped expanding FastAPI router includes after {MAX_ROUTER_CONTEXTS} contexts"
            ),
            source: None,
        });
    }
    for _ in 0..64 {
        let snapshot = contexts.clone();
        let mut changed = false;
        for ((parent_path, parent_router), parent_contexts) in snapshot {
            let Some(parent_file) = parsed.get(&parent_path) else {
                continue;
            };
            for include in parent_file
                .includes
                .iter()
                .filter(|include| include.parent == parent_router)
            {
                let Some(child_file) = parsed.get(&include.child_file) else {
                    continue;
                };
                let Some(child_router) = select_child_router(child_file, &include.child_hint)
                else {
                    continue;
                };
                for parent_context in &parent_contexts {
                    let mut dependencies = parent_context.dependencies.clone();
                    dependencies.extend(include.dependencies.iter().cloned());
                    dependencies.extend(child_router.dependencies.iter().cloned());
                    dependencies.extend(
                        child_file
                            .middleware
                            .get(&child_router.name)
                            .into_iter()
                            .flatten()
                            .cloned(),
                    );
                    let context = RouterContext {
                        prefix: join_paths(
                            &join_paths(&parent_context.prefix, &include.prefix),
                            &child_router.prefix,
                        ),
                        dependencies,
                    };
                    let target = contexts
                        .entry((include.child_file.clone(), child_router.name.clone()))
                        .or_default();
                    if !target.contains(&context) && context_count >= MAX_ROUTER_CONTEXTS {
                        if !context_budget_reported {
                            context_budget_reported = true;
                            output.diagnostics.push(Diagnostic {
                                code: "fastapi-context-budget".to_owned(),
                                severity: DiagnosticSeverity::Warning,
                                message: format!(
                                    "Stopped expanding FastAPI router includes after {MAX_ROUTER_CONTEXTS} contexts"
                                ),
                                source: None,
                            });
                        }
                        continue;
                    }
                    if target.insert(context) {
                        context_count += 1;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    for (path, file) in &parsed {
        for route in &file.routes {
            let route_contexts = contexts
                .get(&(path.clone(), route.router.clone()))
                .map_or_else(
                    || vec![RouterContext::default()],
                    |contexts| contexts.iter().cloned().collect::<Vec<_>>(),
                );
            for context in route_contexts {
                let full_path = join_paths(&context.prefix, &route.path);
                let mut dependencies = context.dependencies;
                dependencies.extend(route.dependencies.iter().cloned());
                dependencies = expand_subdependencies(&dependencies, &function_dependencies);
                let source = SourceRef {
                    file: file.file.relative.clone(),
                    line: file.locator.line(route.offset),
                    column: 1,
                };
                for method in &route.methods {
                    let explicit = dependencies
                        .iter()
                        .map(|dependency| ExplicitDependency {
                            name: dependency.name.clone(),
                            kind: DependencyKind::Middleware,
                            confidence: Confidence::Exact,
                            evidence: Evidence {
                                kind: EvidenceKind::Framework,
                                detail: format!("FastAPI resolved Depends({})", dependency.name),
                                source: Some(source.clone()),
                            },
                            pinned: false,
                            packages: Vec::new(),
                        })
                        .collect();
                    output.routes.push(RouteRecord {
                        endpoint: Endpoint {
                            id: canonical_endpoint_id(method, &full_path),
                            method: method.clone(),
                            path: full_path.clone(),
                            display_path: full_path.clone(),
                            district: district_for_path(&full_path),
                            framework: "fastapi".to_owned(),
                            operation_id: None,
                            tags: Vec::new(),
                            sources: vec![source.clone()],
                            spec_only: false,
                            contract: None,
                        },
                        source_path: file.file.path.clone(),
                        entry_symbols: route.entry_symbol.iter().cloned().collect(),
                        inline_code: Vec::new(),
                        dependencies: explicit,
                    });
                }
            }
        }
    }
    output.routes.sort_by(|left, right| {
        left.endpoint
            .path
            .cmp(&right.endpoint.path)
            .then_with(|| left.endpoint.method.cmp(&right.endpoint.method))
    });
    output
}

fn parse_file<'a>(file: &'a PythonFile, diagnostics: &mut Vec<Diagnostic>) -> ParsedFile<'a> {
    let locator = SourceLocator::new(&file.source);
    let mut scan_budget = CallScanBudget {
        remaining: MAX_CALL_SCAN_WORK,
        exhausted: false,
    };
    let routers = extract_router_definitions(file, &mut scan_budget);
    let route_declarations = extract_routes(file, diagnostics, &locator, &mut scan_budget);
    let includes = extract_includes(file, &routers, diagnostics, &locator, &mut scan_budget);
    let middleware = extract_middleware(file);
    if scan_budget.exhausted {
        diagnostics.push(Diagnostic {
            code: "fastapi-call-budget".to_owned(),
            severity: DiagnosticSeverity::Warning,
            message: format!(
                "Stopped scanning malformed or oversized FastAPI calls in {} after {MAX_CALL_SCAN_WORK} bytes of work",
                file.relative
            ),
            source: Some(SourceRef {
                file: file.relative.clone(),
                line: 1,
                column: 1,
            }),
        });
    }
    ParsedFile {
        file,
        locator,
        routers,
        routes: route_declarations,
        includes,
        middleware,
    }
}

fn extract_router_definitions(
    file: &PythonFile,
    scan_budget: &mut CallScanBudget,
) -> Vec<RouterDefinition> {
    static ROUTER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^\s*([A-Za-z_]\w*)\s*=\s*(FastAPI|APIRouter)\s*\(")
            .expect("valid FastAPI router regex")
    });
    let mut routers = Vec::new();
    for captures in ROUTER.captures_iter(&file.source) {
        let whole = captures.get(0).expect("whole router capture");
        let open = whole.end() - 1;
        let body = call_body(&file.source, open, scan_budget).map_or("", |(body, _)| body);
        routers.push(RouterDefinition {
            name: captures
                .get(1)
                .map_or("router", |value| value.as_str())
                .to_owned(),
            prefix: named_string_argument(body, "prefix").unwrap_or_default(),
            dependencies: extract_depends(file, body),
            is_app: captures
                .get(2)
                .is_some_and(|value| value.as_str() == "FastAPI"),
        });
    }
    routers
}

fn extract_routes(
    file: &PythonFile,
    diagnostics: &mut Vec<Diagnostic>,
    locator: &SourceLocator,
    scan_budget: &mut CallScanBudget,
) -> Vec<RawRoute> {
    static DECORATOR: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?m)^\s*@([A-Za-z_]\w*)\.(get|post|put|patch|delete|options|head|api_route)\s*\(",
        )
        .expect("valid FastAPI decorator regex")
    });
    let mut routes = Vec::new();
    for captures in DECORATOR.captures_iter(&file.source) {
        let whole = captures.get(0).expect("whole decorator capture");
        let open = whole.end() - 1;
        let Some((body, close)) = call_body(&file.source, open, scan_budget) else {
            continue;
        };
        let Some(path) = first_string_argument(body) else {
            diagnostics.push(Diagnostic {
                code: "fastapi-dynamic-path".to_owned(),
                severity: DiagnosticSeverity::Warning,
                message: format!("Skipped a computed FastAPI route path in {}", file.relative),
                source: Some(SourceRef {
                    file: file.relative.clone(),
                    line: locator.line(whole.start()),
                    column: 1,
                }),
            });
            continue;
        };
        let decorator_method = captures.get(2).map_or("get", |value| value.as_str());
        let methods = if decorator_method == "api_route" {
            extract_methods(body)
        } else {
            vec![decorator_method.to_ascii_uppercase()]
        };
        if methods.is_empty() {
            diagnostics.push(Diagnostic {
                code: "fastapi-dynamic-methods".to_owned(),
                severity: DiagnosticSeverity::Warning,
                message: format!(
                    "Skipped api_route with computed methods in {}",
                    file.relative
                ),
                source: Some(SourceRef {
                    file: file.relative.clone(),
                    line: locator.line(whole.start()),
                    column: 1,
                }),
            });
            continue;
        }
        let function = following_function(&file.source, close + 1);
        let function_region = function.as_ref().map(|(_, region)| *region);
        let mut dependencies = extract_depends(file, body);
        if let Some(region) = function_region {
            dependencies.extend(extract_depends(file, region));
        }
        routes.push(RawRoute {
            router: captures
                .get(1)
                .map_or("app", |value| value.as_str())
                .to_owned(),
            methods,
            path: normalize_route_path(&path),
            offset: whole.start(),
            dependencies,
            entry_symbol: function.map(|(name, _)| name),
        });
    }
    routes
}

fn extract_includes(
    file: &PythonFile,
    routers: &[RouterDefinition],
    diagnostics: &mut Vec<Diagnostic>,
    locator: &SourceLocator,
    scan_budget: &mut CallScanBudget,
) -> Vec<RouterInclude> {
    static INCLUDE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\b([A-Za-z_]\w*)\.include_router\s*\(").expect("valid include_router regex")
    });
    let mut includes = Vec::new();
    for captures in INCLUDE.captures_iter(&file.source) {
        let whole = captures.get(0).expect("whole include capture");
        let open = whole.end() - 1;
        let Some((body, _)) = call_body(&file.source, open, scan_budget) else {
            continue;
        };
        let first = split_top_level(body).first().copied().unwrap_or_default();
        let root_symbol = first.split('.').next().unwrap_or(first).trim();
        let child_hint = first
            .split('.')
            .next_back()
            .unwrap_or("router")
            .trim()
            .to_owned();
        let child_file = if routers.iter().any(|router| router.name == root_symbol) {
            Some(file.path.clone())
        } else {
            file.imports
                .iter()
                .find(|import| import.locals.iter().any(|local| local == root_symbol))
                .and_then(|import| import.resolved.clone())
        };
        let Some(child_file) = child_file else {
            diagnostics.push(Diagnostic {
                code: "fastapi-unresolved-router".to_owned(),
                severity: DiagnosticSeverity::Warning,
                message: format!(
                    "Could not resolve include_router target in {}",
                    file.relative
                ),
                source: Some(SourceRef {
                    file: file.relative.clone(),
                    line: locator.line(whole.start()),
                    column: 1,
                }),
            });
            continue;
        };
        includes.push(RouterInclude {
            parent: captures
                .get(1)
                .map_or("app", |value| value.as_str())
                .to_owned(),
            child_file,
            child_hint,
            prefix: named_string_argument(body, "prefix").unwrap_or_default(),
            dependencies: extract_depends(file, body),
        });
    }
    includes
}

fn extract_middleware(file: &PythonFile) -> BTreeMap<String, BTreeSet<DependencyRef>> {
    static MIDDLEWARE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?m)^\s*@([A-Za-z_]\w*)\.middleware\s*\(\s*[\"']http[\"']\s*\)\s*\n\s*(?:async\s+)?def\s+([A-Za-z_]\w*)"#,
        )
        .expect("valid FastAPI middleware regex")
    });
    let mut output = BTreeMap::<String, BTreeSet<DependencyRef>>::new();
    for captures in MIDDLEWARE.captures_iter(&file.source) {
        let router = captures.get(1).map_or("app", |value| value.as_str());
        let name = captures
            .get(2)
            .map_or("http middleware", |value| value.as_str());
        output
            .entry(router.to_owned())
            .or_default()
            .insert(DependencyRef {
                name: name.to_owned(),
                node: Some(DependencyNode {
                    file: file.path.clone(),
                    symbol: name.to_owned(),
                }),
            });
    }
    output
}

fn collect_function_dependencies(
    index: &PythonIndex,
) -> BTreeMap<DependencyNode, BTreeSet<DependencyRef>> {
    static FUNCTION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^([ \t]*)(?:async\s+)?def\s+([A-Za-z_]\w*)\s*\(")
            .expect("valid Python function regex")
    });
    let mut output = BTreeMap::<DependencyNode, BTreeSet<DependencyRef>>::new();
    for file in index.files() {
        for captures in FUNCTION.captures_iter(&file.source) {
            let whole = captures.get(0).expect("whole function capture");
            let name = captures.get(2).map_or("", |value| value.as_str());
            if let Some(region) = function_region_at(&file.source, whole.start()) {
                output
                    .entry(DependencyNode {
                        file: file.path.clone(),
                        symbol: name.to_owned(),
                    })
                    .or_default()
                    .extend(extract_depends(file, region));
            }
        }
    }
    output
}

fn expand_subdependencies(
    initial: &BTreeSet<DependencyRef>,
    graph: &BTreeMap<DependencyNode, BTreeSet<DependencyRef>>,
) -> BTreeSet<DependencyRef> {
    let mut output = BTreeSet::new();
    let mut queue = initial.iter().cloned().collect::<VecDeque<_>>();
    while let Some(dependency) = queue.pop_front() {
        if output.len() >= MAX_DEPENDENCIES_PER_ROUTE {
            break;
        }
        if !output.insert(dependency.clone()) {
            continue;
        }
        if let Some(children) = dependency.node.as_ref().and_then(|node| graph.get(node)) {
            queue.extend(children.iter().cloned());
        }
    }
    output
}

fn select_child_router<'a>(file: &'a ParsedFile<'_>, hint: &str) -> Option<&'a RouterDefinition> {
    file.routers
        .iter()
        .find(|router| router.name == hint)
        .or_else(|| file.routers.iter().find(|router| !router.is_app))
}

fn call_body<'a>(
    source: &'a str,
    open: usize,
    budget: &mut CallScanBudget,
) -> Option<(&'a str, usize)> {
    let bytes = source.as_bytes();
    if bytes.get(open) != Some(&b'(') || budget.remaining == 0 {
        budget.exhausted |= budget.remaining == 0;
        return None;
    }
    let scan_limit = open
        .saturating_add(MAX_CALL_BODY_BYTES)
        .saturating_add(1)
        .min(bytes.len())
        .min(open.saturating_add(budget.remaining));
    let scan_truncated = scan_limit < bytes.len();
    let mut depth = 0_u32;
    let mut quote = None;
    let mut escaped = false;
    for index in open..scan_limit {
        let byte = bytes[index];
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == active_quote {
                quote = None;
            }
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'(' => depth += 1,
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    budget.remaining = budget
                        .remaining
                        .saturating_sub(index.saturating_sub(open).saturating_add(1));
                    return Some((&source[open + 1..index], index));
                }
            }
            _ => {}
        }
    }
    budget.remaining = budget
        .remaining
        .saturating_sub(scan_limit.saturating_sub(open));
    budget.exhausted |= budget.remaining == 0 || scan_truncated;
    None
}

fn split_top_level(body: &str) -> Vec<&str> {
    let mut output = Vec::new();
    let mut start = 0;
    let mut depth = 0_i32;
    let mut quote = None;
    let mut escaped = false;
    for (index, byte) in body.bytes().enumerate() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == active_quote {
                quote = None;
            }
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                output.push(body[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    if start < body.len() {
        output.push(body[start..].trim());
    }
    output
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect()
}

fn first_string_argument(body: &str) -> Option<String> {
    parse_python_string(split_top_level(body).first().copied()?)
}

fn named_string_argument(body: &str, name: &str) -> Option<String> {
    split_top_level(body).into_iter().find_map(|argument| {
        let (argument_name, value) = argument.split_once('=')?;
        (argument_name.trim() == name)
            .then(|| parse_python_string(value.trim()))
            .flatten()
    })
}

fn parse_python_string(value: &str) -> Option<String> {
    let value = value.trim();
    let quote = value.as_bytes().first().copied()?;
    if !matches!(quote, b'\'' | b'"') || value.as_bytes().last().copied() != Some(quote) {
        return None;
    }
    Some(
        value[1..value.len().saturating_sub(1)]
            .replace("\\'", "'")
            .replace("\\\"", "\"")
            .replace("\\/", "/"),
    )
}

fn extract_methods(body: &str) -> Vec<String> {
    static METHODS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)\bmethods\s*=\s*\[([^\]]*)\]").expect("valid methods regex")
    });
    static STRING: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"[\"']([A-Za-z]+)[\"']"#).expect("valid method string regex")
    });
    let mut methods = METHODS
        .captures(body)
        .and_then(|captures| captures.get(1))
        .map(|values| {
            STRING
                .captures_iter(values.as_str())
                .filter_map(|captures| {
                    captures
                        .get(1)
                        .map(|value| value.as_str().to_ascii_uppercase())
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    methods.sort();
    methods.dedup();
    methods
}

fn extract_depends(file: &PythonFile, source: &str) -> BTreeSet<DependencyRef> {
    extract_dependency_names(source)
        .into_iter()
        .map(|name| resolve_dependency(file, &name))
        .collect()
}

fn extract_dependency_names(source: &str) -> BTreeSet<String> {
    static DEPENDS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\bDepends\s*\(\s*([A-Za-z_]\w*(?:\.[A-Za-z_]\w*)*)")
            .expect("valid Depends regex")
    });
    DEPENDS
        .captures_iter(source)
        .filter_map(|captures| captures.get(1).map(|value| value.as_str().to_owned()))
        .collect()
}

fn resolve_dependency(file: &PythonFile, name: &str) -> DependencyRef {
    let root = name.split('.').next().unwrap_or(name);
    let imported = file.imports.iter().find_map(|import| {
        let binding = import
            .bindings
            .iter()
            .find(|binding| binding.local == root)?;
        let resolved = import.resolved.clone()?;
        let symbol = name.split_once('.').map_or_else(
            || binding.imported.clone(),
            |(_, member)| member.rsplit('.').next().unwrap_or(member).to_owned(),
        );
        (symbol != "*").then_some(DependencyNode {
            file: resolved,
            symbol,
        })
    });
    let node = imported.or_else(|| {
        (!name.contains('.')).then(|| DependencyNode {
            file: file.path.clone(),
            symbol: name.to_owned(),
        })
    });
    DependencyRef {
        name: name.rsplit('.').next().unwrap_or(name).to_owned(),
        node,
    }
}

fn following_function(source: &str, after: usize) -> Option<(String, &str)> {
    static FUNCTION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^[ \t]*(?:async\s+)?def\s+([A-Za-z_]\w*)\s*\(")
            .expect("valid following function regex")
    });
    let tail = source.get(after..)?;
    let captures = FUNCTION.captures(tail)?;
    let function = captures.get(0)?;
    let name = captures.get(1)?.as_str().to_owned();
    let region = function_region_at(source, after + function.start())?;
    Some((name, region))
}

fn function_region_at(source: &str, start: usize) -> Option<&str> {
    let line_start = source[..start].rfind('\n').map_or(0, |index| index + 1);
    let line = source.get(line_start..)?.lines().next()?;
    let indent = line
        .chars()
        .take_while(|character| character.is_whitespace())
        .count();
    let mut end = source.len();
    let mut offset = line_start + line.len() + 1;
    for next_line in source.get(offset..)?.split_inclusive('\n') {
        let trimmed = next_line.trim();
        let next_indent = next_line
            .chars()
            .take_while(|character| character.is_whitespace())
            .count();
        if !trimmed.is_empty() && !trimmed.starts_with('#') && next_indent <= indent {
            end = offset;
            break;
        }
        offset += next_line.len();
    }
    source.get(line_start..end)
}

fn join_paths(prefix: &str, path: &str) -> String {
    if prefix.is_empty() || prefix == "/" {
        return normalize_route_path(path);
    }
    if path.is_empty() || path == "/" {
        return normalize_route_path(prefix);
    }
    normalize_route_path(&format!(
        "{}/{}",
        prefix.trim_end_matches('/'),
        path.trim_start_matches('/')
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::{
        CallScanBudget, DependencyNode, DependencyRef, MAX_CALL_BODY_BYTES, MAX_CALL_SCAN_WORK,
        call_body, expand_subdependencies, extract_dependency_names, extract_methods, join_paths,
    };

    #[test]
    fn extracts_annotated_and_regular_dependencies() {
        let source = "user: Annotated[User, Depends(current_user)], db=Depends(get_db)";
        assert_eq!(
            extract_dependency_names(source),
            BTreeSet::from(["current_user".to_owned(), "get_db".to_owned()])
        );
    }

    #[test]
    fn resolves_cycle_safe_subdependencies() {
        let file = std::path::PathBuf::from("dependencies.py");
        let dependency = |name: &str| DependencyRef {
            name: name.to_owned(),
            node: Some(DependencyNode {
                file: file.clone(),
                symbol: name.to_owned(),
            }),
        };
        let graph = BTreeMap::from([
            (
                DependencyNode {
                    file: file.clone(),
                    symbol: "auth".to_owned(),
                },
                BTreeSet::from([dependency("session")]),
            ),
            (
                DependencyNode {
                    file: file.clone(),
                    symbol: "session".to_owned(),
                },
                BTreeSet::from([dependency("auth")]),
            ),
        ]);
        assert_eq!(
            expand_subdependencies(&BTreeSet::from([dependency("auth")]), &graph).len(),
            2
        );
    }

    #[test]
    fn expands_api_route_methods_and_prefixes() {
        assert_eq!(extract_methods("methods=['GET', 'POST']"), ["GET", "POST"]);
        assert_eq!(join_paths("/api/v1", "/users/{id}"), "/api/v1/users/{id}");
    }

    #[test]
    fn bounds_unterminated_call_scans() {
        let source = format!("({}", "x".repeat(MAX_CALL_BODY_BYTES + 16));
        let mut budget = CallScanBudget {
            remaining: MAX_CALL_SCAN_WORK,
            exhausted: false,
        };
        assert!(call_body(&source, 0, &mut budget).is_none());
        assert!(budget.exhausted);
        assert!(budget.remaining < MAX_CALL_SCAN_WORK);
    }
}
