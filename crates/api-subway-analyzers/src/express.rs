use std::{
    collections::{BTreeMap, BTreeSet},
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
    javascript::{JsFile, JsIndex, SourceLocator},
};

const MAX_CONTEXTS: usize = 10_000;
const MAX_HANDLER_DEPTH: usize = 32;
const MAX_HANDLERS_PER_REGISTRATION: usize = 1_000;

#[derive(Debug, Clone)]
struct RawRoute {
    receiver: String,
    method: String,
    path: String,
    offset: usize,
    middleware: Vec<String>,
    entry_symbols: Vec<String>,
    inline_code: Vec<String>,
}

#[derive(Debug, Clone)]
struct RawUse {
    receiver: String,
    prefix: String,
    offset: usize,
    middleware: Vec<String>,
    child: Option<PathBuf>,
}

#[derive(Debug)]
struct ParsedFile<'a> {
    file: &'a JsFile,
    locator: SourceLocator,
    routes: Vec<RawRoute>,
    uses: Vec<RawUse>,
    is_app: bool,
}

#[derive(Debug, Default)]
struct ExpressBindings {
    apps: BTreeSet<String>,
    routers: BTreeSet<String>,
}

impl ExpressBindings {
    fn contains(&self, name: &str) -> bool {
        self.apps.contains(name) || self.routers.contains(name)
    }

    fn is_empty(&self) -> bool {
        self.apps.is_empty() && self.routers.is_empty()
    }
}

pub(crate) fn analyze(_root: &Path, index: &JsIndex) -> AdapterOutput {
    let mut output = AdapterOutput::empty();
    let mut parsed = BTreeMap::<PathBuf, ParsedFile<'_>>::new();
    for file in index.files() {
        let bindings = express_bindings(file);
        if bindings.is_empty() {
            continue;
        }
        let parsed_file = parse_file(file, index, &bindings, &mut output.diagnostics);
        if !parsed_file.routes.is_empty() || !parsed_file.uses.is_empty() {
            parsed.insert(file.path.clone(), parsed_file);
        }
    }

    let mut contexts = BTreeMap::<PathBuf, BTreeMap<String, BTreeSet<String>>>::new();
    for (path, file) in &parsed {
        if file.is_app {
            contexts
                .entry(path.clone())
                .or_default()
                .entry(String::new())
                .or_default();
        }
    }
    let has_application = !contexts.is_empty();
    if contexts.is_empty() {
        for (path, file) in &parsed {
            if !file.routes.is_empty() {
                contexts
                    .entry(path.clone())
                    .or_default()
                    .entry(String::new())
                    .or_default();
            }
        }
    }

    let mut context_count = contexts.values().map(BTreeMap::len).sum::<usize>();
    let mut context_budget_reported = false;
    for _ in 0..64 {
        let snapshot = contexts.clone();
        let mut changed = false;
        for (parent_path, prefixes) in &snapshot {
            let Some(parent) = parsed.get(parent_path) else {
                continue;
            };
            for mount in parent.uses.iter().filter(|mount| mount.child.is_some()) {
                let child = mount.child.as_ref().expect("filtered child mount");
                if !parsed.contains_key(child) {
                    continue;
                }
                for (parent_prefix, inherited) in prefixes {
                    let next_prefix = join_paths(parent_prefix, &mount.prefix);
                    let mut next_middleware = inherited.clone();
                    next_middleware.extend(mount.middleware.iter().cloned());
                    next_middleware.extend(prior_use_middleware(
                        parent,
                        mount.offset,
                        &mount.prefix,
                        &mount.receiver,
                    ));
                    let target_contexts = contexts.entry(child.clone()).or_default();
                    let is_new_context = !target_contexts.contains_key(&next_prefix);
                    if is_new_context && context_count >= MAX_CONTEXTS {
                        if !context_budget_reported {
                            context_budget_reported = true;
                            output.diagnostics.push(Diagnostic {
                                code: "express-context-budget".to_owned(),
                                severity: DiagnosticSeverity::Warning,
                                message: format!(
                                    "Stopped expanding Express router mounts after {MAX_CONTEXTS} contexts"
                                ),
                                source: None,
                            });
                        }
                        continue;
                    }
                    let target = target_contexts.entry(next_prefix).or_default();
                    if is_new_context {
                        context_count += 1;
                    }
                    let previous_len = target.len();
                    target.extend(
                        next_middleware
                            .into_iter()
                            .take(MAX_HANDLERS_PER_REGISTRATION),
                    );
                    changed |= is_new_context || target.len() != previous_len;
                }
            }
        }
        if !changed {
            break;
        }
    }

    for (path, file) in &parsed {
        let Some(file_contexts) = contexts.get(path).cloned().or_else(|| {
            (!has_application).then(|| BTreeMap::from([(String::new(), BTreeSet::new())]))
        }) else {
            continue;
        };
        for (prefix, inherited) in file_contexts {
            for route in &file.routes {
                let path = join_paths(&prefix, &route.path);
                let method = if route.method == "all" {
                    "ANY".to_owned()
                } else {
                    route.method.to_ascii_uppercase()
                };
                let mut middleware = inherited.clone();
                middleware.extend(route.middleware.iter().cloned());
                middleware.extend(prior_use_middleware(
                    file,
                    route.offset,
                    &route.path,
                    &route.receiver,
                ));
                let source = SourceRef {
                    file: file.file.relative.clone(),
                    line: file.locator.line(route.offset),
                    column: 1,
                };
                let dependencies = middleware
                    .into_iter()
                    .map(|name| ExplicitDependency {
                        name: display_handler_name(&name),
                        kind: DependencyKind::Middleware,
                        confidence: Confidence::Exact,
                        evidence: Evidence {
                            kind: EvidenceKind::Framework,
                            detail: format!(
                                "Express registered middleware '{name}' before the handler"
                            ),
                            source: Some(source.clone()),
                        },
                        pinned: false,
                        packages: Vec::new(),
                    })
                    .collect();
                output.routes.push(RouteRecord {
                    endpoint: Endpoint {
                        id: canonical_endpoint_id(&method, &path),
                        method,
                        path: path.clone(),
                        display_path: path.clone(),
                        district: district_for_path(&path),
                        framework: "express".to_owned(),
                        operation_id: None,
                        tags: Vec::new(),
                        sources: vec![source],
                        spec_only: false,
                        contract: None,
                    },
                    source_path: file.file.path.clone(),
                    entry_symbols: route.entry_symbols.clone(),
                    inline_code: route.inline_code.clone(),
                    dependencies,
                });
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

fn parse_file<'a>(
    file: &'a JsFile,
    index: &JsIndex,
    bindings: &ExpressBindings,
    diagnostics: &mut Vec<Diagnostic>,
) -> ParsedFile<'a> {
    static MEMBER_CALL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"\b([A-Za-z_$][\w$]*)\s*\.\s*(get|post|put|patch|delete|options|head|all|use)\s*\(",
        )
        .expect("valid Express member call regex")
    });
    static ROUTE_CHAIN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\b([A-Za-z_$][\w$]*)\s*\.\s*route\s*\(")
            .expect("valid Express route chain regex")
    });
    let mut routes = Vec::new();
    let mut uses = Vec::new();
    let locator = SourceLocator::new(&file.source);
    for captures in MEMBER_CALL.captures_iter(&file.source) {
        let whole = captures.get(0).expect("whole member call capture");
        let receiver = captures.get(1).map_or("", |value| value.as_str());
        if !bindings.contains(receiver) {
            continue;
        }
        let method = captures.get(2).map_or("", |value| value.as_str());
        let open = whole.end() - 1;
        let Some((body, _close)) = call_body(&file.source, open) else {
            continue;
        };
        let args = split_top_level(body);
        if method == "use" {
            let static_prefix = args
                .first()
                .and_then(|argument| resolve_static_path(file, argument));
            if static_prefix.is_none()
                && args.len() > 1
                && !is_handler_argument(file, index, args[0])
            {
                diagnostics.push(dynamic_mount_diagnostic(file, whole.start(), &locator));
                continue;
            }
            let (prefix, handler_start) =
                static_prefix.map_or_else(|| ("/".to_owned(), 0), |path| (path, 1));
            let handler_names = args[handler_start..]
                .iter()
                .flat_map(|argument| handler_names(argument))
                .take(MAX_HANDLERS_PER_REGISTRATION)
                .collect::<Vec<_>>();
            let child_name = handler_names.last();
            let child = child_name
                .and_then(|name| resolve_imported_handler(file, name))
                .filter(|path| {
                    index
                        .get(path)
                        .is_some_and(|target| !express_bindings(target).routers.is_empty())
                });
            let middleware = if child.is_some() {
                handler_names[..handler_names.len().saturating_sub(1)].to_vec()
            } else {
                handler_names
            };
            uses.push(RawUse {
                receiver: receiver.to_owned(),
                prefix,
                offset: whole.start(),
                middleware,
                child,
            });
            continue;
        }
        let Some(path_argument) = args.first() else {
            continue;
        };
        let Some(path) = resolve_static_path(file, path_argument) else {
            diagnostics.push(dynamic_path_diagnostic(file, whole.start(), &locator));
            continue;
        };
        let handlers = args[1..]
            .iter()
            .flat_map(|argument| handler_names(argument))
            .take(MAX_HANDLERS_PER_REGISTRATION)
            .collect::<Vec<_>>();
        let middleware = handlers[..handlers.len().saturating_sub(1)].to_vec();
        let last_argument = args.last().copied().unwrap_or_default();
        let (entry_symbols, inline_code) = analysis_entry(last_argument);
        routes.push(RawRoute {
            receiver: receiver.to_owned(),
            method: method.to_owned(),
            path,
            offset: whole.start(),
            middleware,
            entry_symbols,
            inline_code,
        });
    }

    for captures in ROUTE_CHAIN.captures_iter(&file.source) {
        let whole = captures.get(0).expect("whole route chain capture");
        let receiver = captures.get(1).map_or("", |value| value.as_str());
        if !bindings.contains(receiver) {
            continue;
        }
        let route_open = whole.end() - 1;
        let Some((route_body, route_close)) = call_body(&file.source, route_open) else {
            continue;
        };
        let Some(path) = split_top_level(route_body)
            .first()
            .and_then(|argument| resolve_static_path(file, argument))
        else {
            diagnostics.push(dynamic_path_diagnostic(file, whole.start(), &locator));
            continue;
        };
        let tail = &file.source[route_close.saturating_add(1)..];
        let mut consumed = 0;
        while let Some((method, method_offset, open)) = next_chain_method(tail, consumed) {
            let Some((body, close)) = call_body(tail, open) else {
                break;
            };
            let handlers = split_top_level(body)
                .iter()
                .flat_map(|argument| handler_names(argument))
                .take(MAX_HANDLERS_PER_REGISTRATION)
                .collect::<Vec<_>>();
            let last_argument = split_top_level(body).last().copied().unwrap_or_default();
            let (entry_symbols, inline_code) = analysis_entry(last_argument);
            routes.push(RawRoute {
                receiver: receiver.to_owned(),
                method,
                path: path.clone(),
                offset: route_close + 1 + method_offset,
                middleware: handlers[..handlers.len().saturating_sub(1)].to_vec(),
                entry_symbols,
                inline_code,
            });
            consumed = close.saturating_add(1);
            if !tail[consumed..].trim_start().starts_with('.') {
                break;
            }
        }
    }
    routes.sort_by_key(|route| route.offset);
    routes.dedup_by(|left, right| {
        left.offset == right.offset && left.method == right.method && left.path == right.path
    });
    uses.sort_by_key(|usage| usage.offset);
    ParsedFile {
        file,
        locator,
        routes,
        uses,
        is_app: !bindings.apps.is_empty(),
    }
}

fn express_bindings(file: &JsFile) -> ExpressBindings {
    static FACTORY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?m)\b(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)(?:\s*:[^=;\n]+)?\s*=\s*([A-Za-z_$][\w$]*)(?:\s*\.\s*(Router))?\s*\(",
        )
        .expect("valid Express factory regex")
    });
    static DIRECT_REQUIRE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?m)\b(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*require\(\s*["']express["']\s*\)(?:\s*\.\s*(Router))?\s*\("#,
        )
        .expect("valid direct Express require regex")
    });
    static ALIAS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?m)\b(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)(?:\s*:[^=;\n]+)?\s*=\s*([A-Za-z_$][\w$]*)\s*(?:;|$)",
        )
        .expect("valid Express alias regex")
    });

    let mut express_factories = BTreeSet::new();
    let mut router_factories = BTreeSet::new();
    for import in &file.imports {
        if import.specifier != "express" {
            continue;
        }
        for binding in &import.bindings {
            match binding.imported.as_str() {
                "default" | "*" => {
                    express_factories.insert(binding.local.clone());
                }
                "Router" => {
                    router_factories.insert(binding.local.clone());
                }
                _ => {}
            }
        }
    }

    let mut bindings = ExpressBindings::default();
    for captures in FACTORY.captures_iter(&file.source) {
        let variable = captures.get(1).map_or("", |value| value.as_str());
        let factory = captures.get(2).map_or("", |value| value.as_str());
        if captures.get(3).is_some() && express_factories.contains(factory)
            || captures.get(3).is_none() && router_factories.contains(factory)
        {
            bindings.routers.insert(variable.to_owned());
        } else if captures.get(3).is_none() && express_factories.contains(factory) {
            bindings.apps.insert(variable.to_owned());
        }
    }
    for captures in DIRECT_REQUIRE.captures_iter(&file.source) {
        let variable = captures.get(1).map_or("", |value| value.as_str());
        if captures.get(2).is_some() {
            bindings.routers.insert(variable.to_owned());
        } else {
            bindings.apps.insert(variable.to_owned());
        }
    }

    for _ in 0..64 {
        let mut changed = false;
        for captures in ALIAS.captures_iter(&file.source) {
            let alias = captures.get(1).map_or("", |value| value.as_str());
            let target = captures.get(2).map_or("", |value| value.as_str());
            if bindings.apps.contains(target) {
                changed |= bindings.apps.insert(alias.to_owned());
            } else if bindings.routers.contains(target) {
                changed |= bindings.routers.insert(alias.to_owned());
            }
        }
        if !changed {
            break;
        }
    }
    bindings
}

fn call_body(source: &str, open: usize) -> Option<(&str, usize)> {
    let bytes = source.as_bytes();
    if bytes.get(open) != Some(&b'(') {
        return None;
    }
    let mut depth = 0_u32;
    let mut quote = None;
    let mut escaped = false;
    let mut index = open;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == active_quote {
                quote = None;
            }
            index += 1;
            continue;
        }
        match byte {
            b'\'' | b'"' | b'`' => quote = Some(byte),
            b'(' => depth += 1,
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some((&source[open + 1..index], index));
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn split_top_level(body: &str) -> Vec<&str> {
    let bytes = body.as_bytes();
    let mut output = Vec::new();
    let mut start = 0;
    let mut nesting = 0_i32;
    let mut quote = None;
    let mut escaped = false;
    for (index, byte) in bytes.iter().copied().enumerate() {
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
            b'\'' | b'"' | b'`' => quote = Some(byte),
            b'(' | b'[' | b'{' => nesting += 1,
            b')' | b']' | b'}' => nesting -= 1,
            b',' if nesting == 0 => {
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

fn parse_path_literal(argument: &str) -> Option<String> {
    let argument = argument.trim();
    if argument.starts_with('`') && argument.ends_with('`') {
        let value = &argument[1..argument.len().saturating_sub(1)];
        return (!value.contains("${")).then(|| normalize_route_path(value));
    }
    if argument.starts_with('"') && argument.ends_with('"') {
        return serde_json::from_str::<String>(argument)
            .ok()
            .map(|value| normalize_route_path(&value));
    }
    if argument.starts_with('\'') && argument.ends_with('\'') {
        let value = argument[1..argument.len().saturating_sub(1)]
            .replace("\\'", "'")
            .replace("\\/", "/");
        return Some(normalize_route_path(&value));
    }
    if argument.starts_with('/') {
        let last_slash = argument.rfind('/')?;
        if last_slash > 0 {
            let pattern = argument[1..last_slash].replace('/', "∕");
            return Some(format!("/~{pattern}"));
        }
    }
    None
}

fn resolve_static_path(file: &JsFile, argument: &str) -> Option<String> {
    static CONSTANT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?m)\b(?:export\s+)?const\s+([A-Za-z_$][\w$]*)(?:\s*:[^=;\n]+)?\s*=\s*([^;\n]+)",
        )
        .expect("valid static path constant regex")
    });
    let mut value = argument.trim();
    let mut visited = BTreeSet::new();
    for _ in 0..16 {
        if let Some(path) = parse_path_literal(value) {
            return Some(path);
        }
        if !is_identifier(value) || !visited.insert(value.to_owned()) {
            return None;
        }
        value = CONSTANT.captures_iter(&file.source).find_map(|captures| {
            (captures.get(1)?.as_str() == value)
                .then(|| captures.get(2).map(|match_| match_.as_str().trim()))
                .flatten()
        })?;
    }
    None
}

fn is_handler_argument(file: &JsFile, index: &JsIndex, argument: &str) -> bool {
    is_handler_argument_at(file, index, argument, 0)
}

fn is_handler_argument_at(file: &JsFile, index: &JsIndex, argument: &str, depth: usize) -> bool {
    if depth >= MAX_HANDLER_DEPTH {
        return false;
    }
    let argument = argument.trim();
    if argument.starts_with('[') && argument.ends_with(']') {
        return split_top_level(&argument[1..argument.len().saturating_sub(1)])
            .into_iter()
            .take(MAX_HANDLERS_PER_REGISTRATION)
            .all(|item| is_handler_argument_at(file, index, item, depth + 1));
    }
    if argument.contains("=>")
        || argument.starts_with("function")
        || argument.starts_with("async function")
    {
        return true;
    }
    let Some(name) = handler_names(argument).into_iter().next() else {
        return false;
    };
    let root = name.split('.').next().unwrap_or(&name);
    if file.scope_for_symbol(root).is_some_and(|scope| {
        scope.contains("=>")
            || scope.contains("function ")
            || scope.trim_start().starts_with("function")
    }) {
        return true;
    }
    file.imports.iter().any(|import| {
        let Some(binding) = import.bindings.iter().find(|binding| binding.local == root) else {
            return false;
        };
        let Some(target) = import.resolved.as_deref().and_then(|path| index.get(path)) else {
            return false;
        };
        if !express_bindings(target).routers.is_empty() {
            return true;
        }
        let symbol = name
            .split_once('.')
            .map_or(binding.imported.as_str(), |(_, member)| member);
        target.scope_for_symbol(symbol).is_some_and(|scope| {
            scope.contains("=>")
                || scope.contains("function ")
                || scope.trim_start().starts_with("function")
        })
    })
}

fn is_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    characters.next().is_some_and(|character| {
        character == '_' || character == '$' || character.is_ascii_alphabetic()
    }) && characters
        .all(|character| character == '_' || character == '$' || character.is_ascii_alphanumeric())
}

fn handler_names(argument: &str) -> Vec<String> {
    handler_names_at(argument, 0)
}

fn handler_names_at(argument: &str, depth: usize) -> Vec<String> {
    static HANDLER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^([A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)*)").expect("valid handler regex")
    });
    let argument = argument.trim();
    if depth >= MAX_HANDLER_DEPTH {
        return Vec::new();
    }
    if argument.starts_with('[') && argument.ends_with(']') {
        return split_top_level(&argument[1..argument.len().saturating_sub(1)])
            .into_iter()
            .flat_map(|item| handler_names_at(item, depth + 1))
            .take(MAX_HANDLERS_PER_REGISTRATION)
            .collect();
    }
    if argument.contains("=>") || argument.starts_with("function") || argument.starts_with("async ")
    {
        return Vec::new();
    }
    HANDLER
        .captures(argument)
        .and_then(|captures| captures.get(1))
        .map(|value| vec![value.as_str().to_owned()])
        .unwrap_or_default()
}

fn analysis_entry(argument: &str) -> (Vec<String>, Vec<String>) {
    let names = handler_names(argument);
    if let Some(name) = names.last() {
        return (vec![name.clone()], Vec::new());
    }
    let argument = argument.trim();
    if argument.contains("=>") || argument.starts_with("function") || argument.starts_with("async ")
    {
        (Vec::new(), vec![argument.to_owned()])
    } else {
        (Vec::new(), Vec::new())
    }
}

fn resolve_imported_handler(file: &JsFile, handler: &str) -> Option<PathBuf> {
    let root = handler.split('.').next().unwrap_or(handler);
    file.imports
        .iter()
        .find(|import| import.locals.iter().any(|local| local == root))
        .and_then(|import| import.resolved.clone())
}

fn prior_use_middleware(
    file: &ParsedFile<'_>,
    offset: usize,
    route_path: &str,
    receiver: &str,
) -> BTreeSet<String> {
    file.uses
        .iter()
        .filter(|usage| {
            usage.offset < offset
                && usage.child.is_none()
                && usage.receiver == receiver
                && path_prefix_matches(&usage.prefix, route_path)
        })
        .flat_map(|usage| usage.middleware.iter().cloned())
        .collect()
}

fn path_prefix_matches(prefix: &str, path: &str) -> bool {
    prefix == "/"
        || path == prefix
        || path.starts_with(&format!("{}/", prefix.trim_end_matches('/')))
}

fn join_paths(prefix: &str, path: &str) -> String {
    if prefix.is_empty() || prefix == "/" {
        return normalize_route_path(path);
    }
    if path == "/" {
        return normalize_route_path(prefix);
    }
    normalize_route_path(&format!(
        "{}/{}",
        prefix.trim_end_matches('/'),
        path.trim_start_matches('/')
    ))
}

fn display_handler_name(name: &str) -> String {
    name.rsplit('.').next().unwrap_or(name).to_owned()
}

fn dynamic_path_diagnostic(file: &JsFile, offset: usize, locator: &SourceLocator) -> Diagnostic {
    Diagnostic {
        code: "express-dynamic-path".to_owned(),
        severity: DiagnosticSeverity::Warning,
        message: format!("Skipped a computed Express route path in {}", file.relative),
        source: Some(SourceRef {
            file: file.relative.clone(),
            line: locator.line(offset),
            column: 1,
        }),
    }
}

fn dynamic_mount_diagnostic(file: &JsFile, offset: usize, locator: &SourceLocator) -> Diagnostic {
    Diagnostic {
        code: "express-dynamic-mount".to_owned(),
        severity: DiagnosticSeverity::Warning,
        message: format!(
            "Skipped an Express router mount with an unresolved path in {}",
            file.relative
        ),
        source: Some(SourceRef {
            file: file.relative.clone(),
            line: locator.line(offset),
            column: 1,
        }),
    }
}

fn next_chain_method(source: &str, start: usize) -> Option<(String, usize, usize)> {
    static CHAIN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^\s*\.\s*(get|post|put|patch|delete|options|head|all)\s*\(")
            .expect("valid route chain regex")
    });
    let tail = source.get(start..)?;
    let captures = CHAIN.captures(tail)?;
    let whole = captures.get(0)?;
    let method = captures.get(1)?.as_str().to_owned();
    Some((method, start + whole.start(), start + whole.end() - 1))
}

#[cfg(test)]
mod tests {
    use super::{call_body, handler_names, join_paths, parse_path_literal, split_top_level};

    #[test]
    fn splits_handler_arrays_without_splitting_calls() {
        assert_eq!(
            split_top_level("'/x', auth, validate(schema), handler"),
            ["'/x'", "auth", "validate(schema)", "handler"]
        );
        assert_eq!(
            handler_names("[auth, validate(schema), handler]"),
            ["auth", "validate", "handler"]
        );
    }

    #[test]
    fn reads_balanced_route_calls() {
        let source = "app.get('/x', validate({ mode: 'a' }), handler);";
        let open = source.find('(').expect("call open");
        assert_eq!(
            call_body(source, open).map(|(body, _)| body),
            Some("'/x', validate({ mode: 'a' }), handler")
        );
    }

    #[test]
    fn handles_literal_and_regexp_paths() {
        assert_eq!(
            parse_path_literal("'/users/:id'"),
            Some("/users/{id}".to_owned())
        );
        assert_eq!(
            parse_path_literal("/^\\/v[12]\\//"),
            Some("/~^\\∕v[12]\\∕".to_owned())
        );
        assert_eq!(parse_path_literal("base + '/users'"), None);
    }

    #[test]
    fn joins_nested_router_mounts() {
        assert_eq!(join_paths("/api", "/users/{id}"), "/api/users/{id}");
    }
}
