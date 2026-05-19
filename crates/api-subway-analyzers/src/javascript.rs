use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
};

use api_subway_core::{
    ApiMapBuilder, Confidence, Dependency, Diagnostic, DiagnosticSeverity, Evidence, EvidenceKind,
    Relation, SourceRef, dependency_id,
};
use oxc_allocator::Allocator;
use oxc_ast::AstKind;
use oxc_parser::Parser;
use oxc_resolver::{ResolveOptions, Resolver};
use oxc_semantic::SemanticBuilder;
use oxc_span::{GetSpan, SourceType, Span};
use rayon::prelude::*;
use regex::Regex;

use crate::{
    boundary, catalog,
    custom_rules::{CompiledDependencyRules, CustomRuleMatch, MAX_CUSTOM_RULE_MATCHES},
    discovery::relative_source_path,
    input::{ReadTextError, read_text_bounded},
};

const MAX_SOURCE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ROUTE_GRAPH_SCOPES: usize = 10_000;
const MAX_ROUTE_GRAPH_QUEUE: usize = 20_000;

#[derive(Debug, Clone)]
pub(crate) struct JsImportBinding {
    pub imported: String,
    pub local: String,
}

#[derive(Debug, Clone)]
pub(crate) struct JsImport {
    pub specifier: String,
    pub locals: Vec<String>,
    pub bindings: Vec<JsImportBinding>,
    pub resolved: Option<PathBuf>,
    pub line: u32,
}

#[derive(Debug, Clone)]
struct JsReExport {
    exported: String,
    imported: String,
    resolved: PathBuf,
    line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CodeRange {
    start: usize,
    end: usize,
}

#[derive(Debug)]
pub(crate) struct SourceLocator {
    line_starts: Vec<usize>,
    ascii: bool,
}

impl SourceLocator {
    pub(crate) fn new(source: &str) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(
            source
                .bytes()
                .enumerate()
                .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
        );
        Self {
            line_starts,
            ascii: source.is_ascii(),
        }
    }

    pub(crate) fn line(&self, byte_offset: usize) -> u32 {
        self.line_index(byte_offset)
            .saturating_add(1)
            .try_into()
            .unwrap_or(u32::MAX)
    }

    pub(crate) fn column(&self, source: &str, byte_offset: usize) -> u32 {
        let offset = byte_offset.min(source.len());
        let line_start = self.line_starts[self.line_index(offset)];
        let column = if self.ascii {
            offset.saturating_sub(line_start)
        } else {
            source[line_start..offset].chars().count()
        };
        column.saturating_add(1).try_into().unwrap_or(u32::MAX)
    }

    fn line_index(&self, byte_offset: usize) -> usize {
        self.line_starts
            .partition_point(|line_start| *line_start <= byte_offset)
            .saturating_sub(1)
    }
}

#[derive(Debug, Clone)]
struct JsCallSite {
    path: Vec<String>,
    offset: usize,
    line: u32,
    column: u32,
    owner: Option<CodeRange>,
    dynamic_member: bool,
}

#[derive(Debug, Clone)]
struct JsAlias {
    alias: String,
    target: String,
    offset: usize,
    owner: Option<CodeRange>,
}

#[derive(Debug, Clone)]
pub(crate) struct JsFile {
    pub path: PathBuf,
    pub relative: String,
    pub source: String,
    pub imports: Vec<JsImport>,
    pub parse_ok: bool,
    reexports: Vec<JsReExport>,
    calls: Vec<JsCallSite>,
    function_ranges: Vec<CodeRange>,
    symbol_ranges: BTreeMap<String, CodeRange>,
    aliases: Vec<JsAlias>,
}

impl JsFile {
    pub(crate) fn scope_for_symbol(&self, symbol: &str) -> Option<String> {
        let range = self
            .symbol_ranges
            .get(symbol)
            .copied()
            .or_else(|| scope_range_for_symbol(&self.source, symbol))?;
        self.source.get(range.start..range.end).map(str::to_owned)
    }

    fn range_for_symbol(&self, symbol: &str) -> Option<CodeRange> {
        self.symbol_ranges
            .get(symbol)
            .copied()
            .or_else(|| scope_range_for_symbol(&self.source, symbol))
    }
}

#[derive(Debug)]
pub(crate) struct JsIndex {
    files: BTreeMap<PathBuf, JsFile>,
    diagnostics: Vec<Diagnostic>,
    custom_rules: Arc<CompiledDependencyRules>,
    custom_matches: BTreeMap<PathBuf, Vec<CustomRuleMatch>>,
}

impl JsIndex {
    pub fn build(
        root: &Path,
        paths: &[PathBuf],
        custom_rules: Arc<CompiledDependencyRules>,
    ) -> Self {
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let resolver = Resolver::new(ResolveOptions {
            extensions: vec![
                ".ts".into(),
                ".tsx".into(),
                ".js".into(),
                ".jsx".into(),
                ".mjs".into(),
                ".cjs".into(),
                ".json".into(),
            ],
            symlinks: false,
            ..ResolveOptions::default()
        });
        let results = paths
            .par_iter()
            .map(|path| index_file(&canonical_root, path, &resolver))
            .collect::<Vec<_>>();
        let mut index = Self {
            files: BTreeMap::new(),
            diagnostics: Vec::new(),
            custom_rules,
            custom_matches: BTreeMap::new(),
        };
        for result in results {
            match result {
                Ok(file) => {
                    index.files.insert(file.path.clone(), file);
                }
                Err(diagnostic) => index.diagnostics.push(diagnostic),
            }
        }
        index.index_custom_rules();
        index.diagnostics.sort();
        index
    }

    fn index_custom_rules(&mut self) {
        if self.custom_rules.is_empty() {
            return;
        }
        let mut matches_by_path = BTreeMap::new();
        let mut match_count = 0_usize;
        let mut truncated_at = None;
        for file in self.files.values() {
            let mut matches = self.custom_rules.match_file(
                &file.relative,
                file.imports
                    .iter()
                    .map(|import| (import.specifier.as_str(), import.line)),
                '/',
            );
            let remaining = MAX_CUSTOM_RULE_MATCHES.saturating_sub(match_count);
            if matches.len() > remaining {
                matches.truncate(remaining);
                truncated_at = Some(file.relative.clone());
            }
            match_count += matches.len();
            if !matches.is_empty() {
                matches_by_path.insert(file.path.clone(), matches);
            }
            if match_count >= MAX_CUSTOM_RULE_MATCHES {
                truncated_at.get_or_insert_with(|| file.relative.clone());
                break;
            }
        }
        self.custom_matches = matches_by_path;
        if let Some(file) = truncated_at {
            self.diagnostics.push(Diagnostic {
                code: "custom-rule-budget".to_owned(),
                severity: DiagnosticSeverity::Warning,
                message: format!(
                    "Stopped indexing configured dependency matches after {MAX_CUSTOM_RULE_MATCHES} file/rule matches"
                ),
                source: Some(SourceRef {
                    file,
                    line: 1,
                    column: 1,
                }),
            });
        }
    }

    pub fn files(&self) -> impl Iterator<Item = &JsFile> {
        self.files.values()
    }

    pub fn get(&self, path: &Path) -> Option<&JsFile> {
        self.files.get(path)
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn classify_route(
        &self,
        endpoint_id: &str,
        start: &Path,
        entry_symbols: &[String],
        inline_code: &[String],
        builder: &mut ApiMapBuilder,
    ) {
        let mut queue = VecDeque::from([ScopeRequest {
            path: start.to_path_buf(),
            symbols: entry_symbols.to_vec(),
            inline_code: inline_code.to_vec(),
            trace: Vec::new(),
        }]);
        let mut visited_scopes = BTreeSet::<(PathBuf, String)>::new();
        let mut visited_paths = BTreeSet::<PathBuf>::new();
        let mut graph_truncated = false;
        while let Some(request) = queue.pop_front() {
            if visited_scopes.len() >= MAX_ROUTE_GRAPH_SCOPES {
                graph_truncated = true;
                break;
            }
            let Some(file) = self.files.get(&request.path) else {
                continue;
            };
            let mut scopes = Vec::new();
            for symbol in request.symbols {
                if !visited_scopes.insert((request.path.clone(), symbol.clone())) {
                    continue;
                }
                if let Some(scope) = scope_region_for_symbol(file, &symbol) {
                    scopes.push(scope);
                } else if let Some((path, target_symbol, line)) = forwarded_symbol(file, &symbol) {
                    let mut trace = request.trace.clone();
                    trace.push(Evidence {
                        kind: EvidenceKind::Import,
                        detail: format!(
                            "Resolved forwarded symbol {symbol} through {}",
                            file.relative
                        ),
                        source: Some(SourceRef {
                            file: file.relative.clone(),
                            line,
                            column: 1,
                        }),
                    });
                    visited_paths.insert(request.path.clone());
                    enqueue_scope(
                        &mut queue,
                        &mut graph_truncated,
                        ScopeRequest {
                            path,
                            symbols: vec![target_symbol],
                            inline_code: Vec::new(),
                            trace,
                        },
                    );
                }
            }
            for (index, inline) in request.inline_code.into_iter().enumerate() {
                let key = format!("<inline:{index}:{}>", stable_scope_key(&inline));
                if visited_scopes.insert((request.path.clone(), key))
                    && let Some(scope) = scope_region_for_inline(file, &inline)
                {
                    scopes.push(scope);
                }
            }
            if scopes.is_empty()
                && request.path == start
                && entry_symbols.is_empty()
                && inline_code.is_empty()
                && visited_scopes.insert((request.path.clone(), "<module>".to_owned()))
            {
                scopes.push(ScopeRegion {
                    range: CodeRange {
                        start: 0,
                        end: file.source.len(),
                    },
                    owner: None,
                });
            }
            if scopes.is_empty() {
                continue;
            }
            visited_paths.insert(request.path.clone());
            let calls = calls_in_scopes(file, &scopes);
            for import in &file.imports {
                let usages = import_usages(file, import, &calls);
                if usages.is_empty() {
                    continue;
                }
                if let Some(entry) = catalog::by_package(&import.specifier) {
                    let dependency_id = dependency_id(entry.kind, entry.name);
                    builder.add_dependency(Dependency {
                        id: dependency_id.clone(),
                        name: entry.name.to_owned(),
                        kind: entry.kind,
                        pinned: false,
                        packages: vec![import.specifier.clone()],
                    });
                    for usage in usages {
                        let mut evidence = request.trace.clone();
                        evidence.push(Evidence {
                            kind: EvidenceKind::Call,
                            detail: format!(
                                "Resolved {}() call through the {} package",
                                usage.call.path.join("."),
                                import.specifier
                            ),
                            source: Some(call_source(file, usage.call)),
                        });
                        builder.add_relation(Relation {
                            endpoint_id: endpoint_id.to_owned(),
                            dependency_id: dependency_id.clone(),
                            confidence: Confidence::Exact,
                            evidence,
                        });
                    }
                } else if let Some(resolved) = &import.resolved {
                    let Some(target) = self.files.get(resolved) else {
                        continue;
                    };
                    for usage in usages {
                        let edge = Evidence {
                            kind: EvidenceKind::Call,
                            detail: format!(
                                "Resolved {}() call into local module {}",
                                usage.call.path.join("."),
                                target.relative
                            ),
                            source: Some(call_source(file, usage.call)),
                        };
                        if let Some(local) = boundary::classify_local_module(
                            &target.relative,
                            usage.target_symbol.as_deref(),
                        ) {
                            let dependency_id = dependency_id(local.kind, &local.name);
                            builder.add_dependency(Dependency {
                                id: dependency_id.clone(),
                                name: local.name,
                                kind: local.kind,
                                pinned: false,
                                packages: Vec::new(),
                            });
                            let mut evidence = request.trace.clone();
                            evidence.push(Evidence {
                                kind: EvidenceKind::Heuristic,
                                detail: format!(
                                    "Resolved {}() call to local {} module {}",
                                    usage.call.path.join("."),
                                    local.role,
                                    target.relative
                                ),
                                source: Some(call_source(file, usage.call)),
                            });
                            builder.add_relation(Relation {
                                endpoint_id: endpoint_id.to_owned(),
                                dependency_id,
                                confidence: Confidence::Inferred,
                                evidence,
                            });
                        }
                        if usage.call.dynamic_member && usage.target_symbol.is_none() {
                            builder.add_diagnostic(Diagnostic {
                                code: "dynamic-dependency-call".to_owned(),
                                severity: DiagnosticSeverity::Warning,
                                message: format!(
                                    "Could not resolve the computed member called through {} in {}",
                                    import.specifier, file.relative
                                ),
                                source: Some(call_source(file, usage.call)),
                            });
                        }
                        if let Some(symbol) = usage.target_symbol {
                            let mut trace = request.trace.clone();
                            trace.push(edge);
                            enqueue_scope(
                                &mut queue,
                                &mut graph_truncated,
                                ScopeRequest {
                                    path: resolved.clone(),
                                    symbols: vec![symbol],
                                    inline_code: Vec::new(),
                                    trace,
                                },
                            );
                        }
                    }
                }
            }
            let mut local_calls = BTreeMap::<String, &JsCallSite>::new();
            for call in calls
                .iter()
                .copied()
                .filter(|call| !call_is_import_bound(file, call))
            {
                if let Some(symbol) = call.path.first()
                    && file.scope_for_symbol(symbol).is_some()
                {
                    local_calls.entry(symbol.clone()).or_insert(call);
                }
            }
            for (symbol, call) in local_calls {
                let mut trace = request.trace.clone();
                trace.push(Evidence {
                    kind: EvidenceKind::Call,
                    detail: format!("Resolved local helper call {}()", call.path.join(".")),
                    source: Some(call_source(file, call)),
                });
                enqueue_scope(
                    &mut queue,
                    &mut graph_truncated,
                    ScopeRequest {
                        path: request.path.clone(),
                        symbols: vec![symbol],
                        inline_code: Vec::new(),
                        trace,
                    },
                );
            }
        }
        if graph_truncated {
            builder.add_diagnostic(Diagnostic {
                code: "javascript-call-graph-budget".to_owned(),
                severity: DiagnosticSeverity::Warning,
                message: format!(
                    "Stopped tracing {endpoint_id} after {MAX_ROUTE_GRAPH_SCOPES} scopes or {MAX_ROUTE_GRAPH_QUEUE} queued edges"
                ),
                source: self.files.get(start).map(|file| SourceRef {
                    file: file.relative.clone(),
                    line: 1,
                    column: 1,
                }),
            });
        }
        let files = visited_paths
            .iter()
            .filter_map(|path| self.files.get(path))
            .collect::<Vec<_>>();
        apply_custom_rules(
            endpoint_id,
            &files,
            &self.custom_rules,
            &self.custom_matches,
            builder,
        );
    }
}

#[derive(Debug)]
struct ScopeRequest {
    path: PathBuf,
    symbols: Vec<String>,
    inline_code: Vec<String>,
    trace: Vec<Evidence>,
}

fn enqueue_scope(queue: &mut VecDeque<ScopeRequest>, truncated: &mut bool, request: ScopeRequest) {
    if queue.len() < MAX_ROUTE_GRAPH_QUEUE {
        queue.push_back(request);
    } else {
        *truncated = true;
    }
}

fn index_file(root: &Path, path: &Path, resolver: &Resolver) -> Result<JsFile, Diagnostic> {
    let relative = relative_source_path(root, path).unwrap_or_else(|| path.display().to_string());
    let source = read_text_bounded(path, MAX_SOURCE_BYTES).map_err(|error| match error {
        ReadTextError::Budget => Diagnostic {
            code: "source-budget".to_owned(),
            severity: DiagnosticSeverity::Warning,
            message: format!("Skipped {relative}: source exceeds the 8 MiB budget"),
            source: None,
        },
        ReadTextError::Io(error) => Diagnostic {
            code: "source-read".to_owned(),
            severity: DiagnosticSeverity::Warning,
            message: format!("Could not read {relative}: {error}"),
            source: None,
        },
    })?;
    let source_type = SourceType::from_path(path).unwrap_or_else(|_| SourceType::mjs());
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &source, source_type).parse();
    let semantic_result = SemanticBuilder::new()
        .with_build_nodes(true)
        .build(&parsed.program);
    let semantic_ok = !parsed.panicked && semantic_result.diagnostics.is_empty();
    let locator = SourceLocator::new(&source);
    let (calls, function_ranges, symbol_ranges) =
        extract_syntax_facts(&source, &semantic_result.semantic, &locator);
    let aliases = extract_aliases(&source, &function_ranges);
    let parse_ok = semantic_ok && parsed.diagnostics.is_empty();
    let imports = extract_imports(root, path, &source, resolver, &locator);
    let reexports = extract_reexports(root, path, &source, resolver, &locator);
    Ok(JsFile {
        path: path.to_path_buf(),
        relative,
        source,
        imports,
        parse_ok,
        reexports,
        calls,
        function_ranges,
        symbol_ranges,
        aliases,
    })
}

fn extract_reexports(
    root: &Path,
    path: &Path,
    source: &str,
    resolver: &Resolver,
    locator: &SourceLocator,
) -> Vec<JsReExport> {
    static NAMED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?m)\bexport\s*\{([^}]*)\}\s*from\s*["']([^"']+)["']"#)
            .expect("valid named re-export regex")
    });
    static STAR: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?m)\bexport\s*\*\s*from\s*["']([^"']+)["']"#)
            .expect("valid star re-export regex")
    });
    let mut output = Vec::new();
    for captures in NAMED.captures_iter(source) {
        let whole = captures.get(0).expect("whole named re-export capture");
        let specifier = captures.get(2).map_or("", |value| value.as_str());
        let Some(target_path) = resolve_workspace_import(root, path, specifier, resolver) else {
            continue;
        };
        let clause = captures.get(1).map_or("", |value| value.as_str());
        for part in clause.split(',') {
            let words = part.split_whitespace().collect::<Vec<_>>();
            let Some(imported) = words
                .first()
                .copied()
                .filter(|value| is_js_identifier(value))
            else {
                continue;
            };
            let exported = if words.get(1) == Some(&"as") {
                words.get(2).copied().unwrap_or(imported)
            } else {
                imported
            };
            if is_js_identifier(exported) {
                output.push(JsReExport {
                    exported: exported.to_owned(),
                    imported: imported.to_owned(),
                    resolved: target_path.clone(),
                    line: locator.line(whole.start()),
                });
            }
        }
    }
    for captures in STAR.captures_iter(source) {
        let whole = captures.get(0).expect("whole star re-export capture");
        let specifier = captures.get(1).map_or("", |value| value.as_str());
        if let Some(resolved) = resolve_workspace_import(root, path, specifier, resolver) {
            output.push(JsReExport {
                exported: "*".to_owned(),
                imported: "*".to_owned(),
                resolved,
                line: locator.line(whole.start()),
            });
        }
    }
    output.sort_by(|left, right| {
        left.exported
            .cmp(&right.exported)
            .then_with(|| left.resolved.cmp(&right.resolved))
            .then_with(|| left.imported.cmp(&right.imported))
    });
    output.dedup_by(|left, right| {
        left.exported == right.exported
            && left.imported == right.imported
            && left.resolved == right.resolved
    });
    output
}

fn forwarded_symbol(file: &JsFile, symbol: &str) -> Option<(PathBuf, String, u32)> {
    let root = symbol.split('.').next().unwrap_or(symbol);
    if let Some((import, binding)) = file.imports.iter().find_map(|import| {
        import
            .bindings
            .iter()
            .find(|binding| binding.local == root)
            .map(|binding| (import, binding))
    }) && let Some(resolved) = &import.resolved
    {
        let target = if binding.imported == "*" {
            symbol
                .split_once('.')
                .map_or(root, |(_, member)| member)
                .to_owned()
        } else {
            binding.imported.clone()
        };
        return Some((resolved.clone(), target, import.line));
    }
    file.reexports
        .iter()
        .find(|reexport| reexport.exported == symbol || reexport.exported == "*")
        .map(|reexport| {
            let target = if reexport.imported == "*" {
                symbol.to_owned()
            } else {
                reexport.imported.clone()
            };
            (reexport.resolved.clone(), target, reexport.line)
        })
}

fn extract_imports(
    root: &Path,
    path: &Path,
    source: &str,
    resolver: &Resolver,
    locator: &SourceLocator,
) -> Vec<JsImport> {
    static IMPORT_FROM: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?m)^[ \t]*import[ \t]+([^;]+?)[ \t\r\n]+from[ \t]+[\"']([^\"']+)[\"']"#)
            .expect("valid import regex")
    });
    static SIDE_EFFECT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?m)^\s*import\s+[\"']([^\"']+)[\"']"#).expect("valid import regex")
    });
    static REQUIRE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?m)^\s*(?:const|let|var)\s+(.+?)\s*=\s*require\(\s*[\"']([^\"']+)[\"']\s*\)"#,
        )
        .expect("valid require regex")
    });
    let mut imports = Vec::new();
    for captures in IMPORT_FROM.captures_iter(source) {
        let whole = captures.get(0).expect("whole import capture");
        let clause = captures.get(1).map_or("", |value| value.as_str());
        let specifier = captures.get(2).map_or("", |value| value.as_str());
        let bindings = parse_import_bindings(clause);
        imports.push(JsImport {
            specifier: specifier.to_owned(),
            locals: bindings
                .iter()
                .map(|binding| binding.local.clone())
                .collect(),
            bindings,
            resolved: resolve_workspace_import(root, path, specifier, resolver),
            line: locator.line(whole.start()),
        });
    }
    for captures in SIDE_EFFECT.captures_iter(source) {
        let whole = captures.get(0).expect("whole import capture");
        let specifier = captures.get(1).map_or("", |value| value.as_str());
        imports.push(JsImport {
            specifier: specifier.to_owned(),
            locals: Vec::new(),
            bindings: Vec::new(),
            resolved: resolve_workspace_import(root, path, specifier, resolver),
            line: locator.line(whole.start()),
        });
    }
    for captures in REQUIRE.captures_iter(source) {
        let whole = captures.get(0).expect("whole require capture");
        let clause = captures.get(1).map_or("", |value| value.as_str());
        let specifier = captures.get(2).map_or("", |value| value.as_str());
        let bindings = parse_require_bindings(clause);
        imports.push(JsImport {
            specifier: specifier.to_owned(),
            locals: bindings
                .iter()
                .map(|binding| binding.local.clone())
                .collect(),
            bindings,
            resolved: resolve_workspace_import(root, path, specifier, resolver),
            line: locator.line(whole.start()),
        });
    }
    imports.sort_by(|left, right| {
        left.line
            .cmp(&right.line)
            .then_with(|| left.specifier.cmp(&right.specifier))
    });
    imports.dedup_by(|left, right| left.line == right.line && left.specifier == right.specifier);
    imports
}

#[cfg(test)]
fn parse_import_locals(clause: &str) -> Vec<String> {
    parse_import_bindings(clause)
        .into_iter()
        .map(|binding| binding.local)
        .collect()
}

fn parse_import_bindings(clause: &str) -> Vec<JsImportBinding> {
    let clause = clause.trim().trim_start_matches("type ").trim();
    let mut output = Vec::new();
    let (default_part, named_part) = if clause.starts_with('{') || clause.starts_with('*') {
        (None, Some(clause))
    } else if let Some((default_part, rest)) = clause.split_once(',') {
        (Some(default_part.trim()), Some(rest.trim()))
    } else {
        (Some(clause), None)
    };
    if let Some(local) = default_part.filter(|value| is_js_identifier(value)) {
        output.push(JsImportBinding {
            imported: "default".to_owned(),
            local: local.to_owned(),
        });
    }
    if let Some(named) = named_part {
        let named = named.trim();
        if let Some(local) = named.strip_prefix("* as ").map(str::trim) {
            if is_js_identifier(local) {
                output.push(JsImportBinding {
                    imported: "*".to_owned(),
                    local: local.to_owned(),
                });
            }
        } else {
            let named = named.trim_matches(|character| character == '{' || character == '}');
            for part in named.split(',') {
                let part = part.trim().trim_start_matches("type ").trim();
                if part.is_empty() {
                    continue;
                }
                let words = part.split_whitespace().collect::<Vec<_>>();
                let imported = words.first().copied().unwrap_or_default();
                let local = if words.get(1) == Some(&"as") {
                    words.get(2).copied().unwrap_or(imported)
                } else {
                    imported
                };
                if is_js_identifier(imported) && is_js_identifier(local) {
                    output.push(JsImportBinding {
                        imported: imported.to_owned(),
                        local: local.to_owned(),
                    });
                }
            }
        }
    }
    output.sort_by(|left, right| left.local.cmp(&right.local));
    output.dedup_by(|left, right| left.local == right.local);
    output
}

fn parse_require_bindings(clause: &str) -> Vec<JsImportBinding> {
    let clause = clause.trim();
    if clause.starts_with('{') && clause.ends_with('}') {
        let mut output = Vec::new();
        for part in clause[1..clause.len().saturating_sub(1)].split(',') {
            let part = part.trim();
            let (imported, local) = part
                .split_once(':')
                .map_or((part, part), |(imported, local)| {
                    (imported.trim(), local.trim())
                });
            if is_js_identifier(imported) && is_js_identifier(local) {
                output.push(JsImportBinding {
                    imported: imported.to_owned(),
                    local: local.to_owned(),
                });
            }
        }
        output.sort_by(|left, right| left.local.cmp(&right.local));
        output.dedup_by(|left, right| left.local == right.local);
        output
    } else if is_js_identifier(clause) {
        vec![JsImportBinding {
            imported: "default".to_owned(),
            local: clause.to_owned(),
        }]
    } else {
        Vec::new()
    }
}

fn is_js_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    characters.next().is_some_and(|character| {
        character == '_' || character == '$' || character.is_ascii_alphabetic()
    }) && characters
        .all(|character| character == '_' || character == '$' || character.is_ascii_alphanumeric())
}

fn resolve_workspace_import(
    root: &Path,
    importer: &Path,
    specifier: &str,
    resolver: &Resolver,
) -> Option<PathBuf> {
    if !(specifier.starts_with('.') || specifier.starts_with('/') || specifier.starts_with("@/")) {
        return None;
    }
    if !lexically_within_root(root, importer, specifier) {
        return None;
    }
    let resolution = resolver.resolve_file(importer, specifier).ok()?;
    let candidate = resolution.full_path();
    let canonical = candidate.canonicalize().ok()?;
    canonical.starts_with(root).then_some(canonical)
}

fn lexically_within_root(root: &Path, importer: &Path, specifier: &str) -> bool {
    use std::path::Component;

    let (mut depth, path) = if let Some(relative) = specifier.strip_prefix("@/") {
        (0_usize, Path::new(relative))
    } else if specifier.starts_with('/') {
        let path = Path::new(specifier);
        let Ok(relative) = path.strip_prefix(root) else {
            return false;
        };
        (0, relative)
    } else {
        let Some(parent) = importer.parent() else {
            return false;
        };
        let Ok(relative_parent) = parent.strip_prefix(root) else {
            return false;
        };
        (relative_parent.components().count(), Path::new(specifier))
    };
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(_) => depth = depth.saturating_add(1),
            Component::ParentDir if depth > 0 => depth -= 1,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

fn extract_aliases(source: &str, function_ranges: &[CodeRange]) -> Vec<JsAlias> {
    static ALIAS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?m)\b(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(?:new\s+)?([A-Za-z_$][\w$]*)\s*(?:\(|\.)",
        )
        .expect("valid JavaScript alias regex")
    });
    let mut aliases = ALIAS
        .captures_iter(source)
        .filter_map(|captures| {
            let whole = captures.get(0)?;
            Some(JsAlias {
                alias: captures.get(1)?.as_str().to_owned(),
                target: captures.get(2)?.as_str().to_owned(),
                offset: whole.start(),
                owner: innermost_containing_function(function_ranges, whole.start()),
            })
        })
        .collect::<Vec<_>>();
    aliases.sort_by_key(|alias| alias.offset);
    aliases
}

#[derive(Debug, Clone, Copy)]
struct ScopeRegion {
    range: CodeRange,
    owner: Option<CodeRange>,
}

#[derive(Debug)]
struct ImportUsage<'a> {
    call: &'a JsCallSite,
    target_symbol: Option<String>,
}

fn extract_syntax_facts(
    source: &str,
    semantic: &oxc_semantic::Semantic<'_>,
    locator: &SourceLocator,
) -> (Vec<JsCallSite>, Vec<CodeRange>, BTreeMap<String, CodeRange>) {
    let nodes = semantic.nodes();
    let mut function_ranges = nodes
        .iter()
        .filter_map(|node| match node.kind() {
            AstKind::Function(function) => code_range(function.span),
            AstKind::ArrowFunctionExpression(function) => code_range(function.span),
            _ => None,
        })
        .collect::<Vec<_>>();
    function_ranges.sort_by_key(|range| (range.start, range.end));
    function_ranges.dedup();

    let mut symbol_ranges = BTreeMap::new();
    for node in nodes.iter() {
        let (range, declared_name, has_body) = match node.kind() {
            AstKind::Function(function) => (
                code_range(function.span),
                function
                    .id
                    .as_ref()
                    .map(|identifier| identifier.name.as_str()),
                function.body.is_some(),
            ),
            AstKind::ArrowFunctionExpression(function) => (code_range(function.span), None, true),
            _ => continue,
        };
        let Some(range) = range.filter(|_| has_body) else {
            continue;
        };
        if let Some(name) = declared_name {
            insert_symbol_range(&mut symbol_ranges, name, range);
        }
        if let Some(name) = nodes.ancestors(node.id()).find_map(|ancestor| {
            let AstKind::VariableDeclarator(declarator) = ancestor.kind() else {
                return None;
            };
            let binding = code_range(declarator.id.span())?;
            let name = source.get(binding.start..binding.end)?;
            is_js_identifier(name).then_some(name)
        }) {
            insert_symbol_range(&mut symbol_ranges, name, range);
        }
        if nodes
            .ancestors(node.id())
            .any(|ancestor| matches!(ancestor.kind(), AstKind::ExportDefaultDeclaration(_)))
        {
            insert_symbol_range(&mut symbol_ranges, "default", range);
        }
    }

    let mut calls = Vec::new();
    for node in nodes.iter() {
        let callee_span = match node.kind() {
            AstKind::CallExpression(call) => Some(call.callee.span()),
            AstKind::NewExpression(call) => Some(call.callee.span()),
            _ => None,
        };
        let Some(callee_span) = callee_span else {
            continue;
        };
        let Some(range) = code_range(callee_span) else {
            continue;
        };
        let Some(callee) = source.get(range.start..range.end) else {
            continue;
        };
        let path = parse_callee_path(callee);
        if path.is_empty() {
            continue;
        }
        let owner = nodes
            .ancestors(node.id())
            .find_map(|ancestor| match ancestor.kind() {
                AstKind::Function(function) => code_range(function.span),
                AstKind::ArrowFunctionExpression(function) => code_range(function.span),
                _ => None,
            });
        calls.push(JsCallSite {
            path,
            offset: range.start,
            line: locator.line(range.start),
            column: locator.column(source, range.start),
            owner,
            dynamic_member: callee.contains('['),
        });
    }
    calls.sort_by_key(|call| call.offset);
    calls.dedup_by(|left, right| left.offset == right.offset && left.path == right.path);
    (calls, function_ranges, symbol_ranges)
}

fn insert_symbol_range(
    ranges: &mut BTreeMap<String, CodeRange>,
    symbol: &str,
    candidate: CodeRange,
) {
    let candidate_size = candidate.end.saturating_sub(candidate.start);
    match ranges.entry(symbol.to_owned()) {
        std::collections::btree_map::Entry::Vacant(entry) => {
            entry.insert(candidate);
        }
        std::collections::btree_map::Entry::Occupied(mut entry) => {
            let current = *entry.get();
            let current_size = current.end.saturating_sub(current.start);
            if candidate_size > current_size
                || (candidate_size == current_size && candidate.start < current.start)
            {
                entry.insert(candidate);
            }
        }
    }
}

fn code_range(span: Span) -> Option<CodeRange> {
    let start = usize::try_from(span.start).ok()?;
    let end = usize::try_from(span.end).ok()?;
    (start <= end).then_some(CodeRange { start, end })
}

fn parse_callee_path(callee: &str) -> Vec<String> {
    let bytes = callee.as_bytes();
    let mut index = 0;
    while index < bytes.len()
        && (bytes[index].is_ascii_whitespace() || matches!(bytes[index], b'(' | b'!'))
    {
        index += 1;
    }
    let Some((root, next)) = read_js_identifier(callee, index) else {
        return Vec::new();
    };
    let mut output = vec![root.to_owned()];
    index = next;
    loop {
        while index < bytes.len()
            && (bytes[index].is_ascii_whitespace() || matches!(bytes[index], b')' | b'!'))
        {
            index += 1;
        }
        if bytes.get(index) == Some(&b'?') && bytes.get(index + 1) == Some(&b'.') {
            index += 2;
        } else if bytes.get(index) == Some(&b'.') {
            index += 1;
        } else {
            break;
        }
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let Some((segment, next)) = read_js_identifier(callee, index) else {
            break;
        };
        output.push(segment.to_owned());
        index = next;
    }
    output
}

fn read_js_identifier(source: &str, start: usize) -> Option<(&str, usize)> {
    let bytes = source.as_bytes();
    let first = *bytes.get(start)?;
    if !(first == b'_' || first == b'$' || first.is_ascii_alphabetic()) {
        return None;
    }
    let mut end = start + 1;
    while bytes
        .get(end)
        .is_some_and(|byte| *byte == b'_' || *byte == b'$' || byte.is_ascii_alphanumeric())
    {
        end += 1;
    }
    Some((&source[start..end], end))
}

fn scope_region_for_symbol(file: &JsFile, symbol: &str) -> Option<ScopeRegion> {
    let range = file.range_for_symbol(symbol)?;
    Some(ScopeRegion {
        range,
        owner: outermost_function_within(&file.function_ranges, range),
    })
}

fn scope_region_for_inline(file: &JsFile, inline: &str) -> Option<ScopeRegion> {
    let start = file.source.find(inline)?;
    let range = CodeRange {
        start,
        end: start.saturating_add(inline.len()),
    };
    Some(ScopeRegion {
        range,
        owner: outermost_function_within(&file.function_ranges, range),
    })
}

fn outermost_function_within(ranges: &[CodeRange], scope: CodeRange) -> Option<CodeRange> {
    ranges
        .iter()
        .copied()
        .filter(|range| range.start >= scope.start && range.end <= scope.end)
        .max_by_key(|range| range.end.saturating_sub(range.start))
}

fn calls_in_scopes<'a>(file: &'a JsFile, scopes: &[ScopeRegion]) -> Vec<&'a JsCallSite> {
    file.calls
        .iter()
        .filter(|call| {
            scopes.iter().any(|scope| {
                call.offset >= scope.range.start
                    && call.offset < scope.range.end
                    && call.owner == scope.owner
            })
        })
        .collect()
}

fn import_usages<'a>(
    file: &JsFile,
    import: &JsImport,
    calls: &[&'a JsCallSite],
) -> Vec<ImportUsage<'a>> {
    let mut output = Vec::new();
    for binding in &import.bindings {
        for call in calls
            .iter()
            .copied()
            .filter(|call| call_uses_binding(file, binding, call))
        {
            let called_through_alias = call.path.first() != Some(&binding.local);
            let target_symbol = if called_through_alias {
                Some(binding.imported.clone())
            } else {
                target_symbol(binding, call)
            };
            output.push(ImportUsage {
                call,
                target_symbol,
            });
        }
    }
    output.sort_by_key(|usage| usage.call.offset);
    output.dedup_by(|left, right| {
        left.call.offset == right.call.offset && left.target_symbol == right.target_symbol
    });
    output
}

fn target_symbol(binding: &JsImportBinding, call: &JsCallSite) -> Option<String> {
    match binding.imported.as_str() {
        "*" => call.path.get(1).cloned(),
        "default" => call
            .path
            .get(1)
            .cloned()
            .or_else(|| Some("default".to_owned())),
        imported => Some(imported.to_owned()),
    }
}

fn call_source(file: &JsFile, call: &JsCallSite) -> SourceRef {
    SourceRef {
        file: file.relative.clone(),
        line: call.line,
        column: call.column,
    }
}

fn call_is_import_bound(file: &JsFile, call: &JsCallSite) -> bool {
    file.imports
        .iter()
        .flat_map(|import| &import.bindings)
        .any(|binding| call_uses_binding(file, binding, call))
}

fn call_uses_binding(file: &JsFile, binding: &JsImportBinding, call: &JsCallSite) -> bool {
    let Some(root) = call.path.first() else {
        return false;
    };
    if root == &binding.local {
        return resolved_alias_target(file, root, call).is_none();
    }
    resolved_alias_target(file, root, call).is_some_and(|target| target == binding.local)
}

fn resolved_alias_target<'a>(file: &'a JsFile, alias: &str, call: &JsCallSite) -> Option<&'a str> {
    file.aliases
        .iter()
        .filter(|candidate| candidate.alias == alias && alias_is_visible(candidate, call))
        .min_by(|left, right| {
            alias_scope_size(left)
                .cmp(&alias_scope_size(right))
                .then_with(|| right.offset.cmp(&left.offset))
        })
        .map(|candidate| candidate.target.as_str())
}

fn alias_is_visible(alias: &JsAlias, call: &JsCallSite) -> bool {
    if alias.offset >= call.offset {
        return false;
    }
    alias.owner.is_none_or(|owner| {
        call.owner.is_some_and(|call_owner| {
            call_owner.start >= owner.start && call_owner.end <= owner.end
        })
    })
}

fn alias_scope_size(alias: &JsAlias) -> usize {
    alias
        .owner
        .map_or(usize::MAX, |range| range.end.saturating_sub(range.start))
}

fn innermost_containing_function(ranges: &[CodeRange], offset: usize) -> Option<CodeRange> {
    ranges
        .iter()
        .copied()
        .filter(|range| offset >= range.start && offset < range.end)
        .min_by_key(|range| range.end.saturating_sub(range.start))
}

#[cfg(test)]
fn scope_for_symbol(source: &str, symbol: &str) -> Option<String> {
    let range = scope_range_for_symbol(source, symbol)?;
    source.get(range.start..range.end).map(str::to_owned)
}

fn scope_range_for_symbol(source: &str, symbol: &str) -> Option<CodeRange> {
    static FUNCTION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?m)\b(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s+([A-Za-z_$][\w$]*)\s*\(",
        )
        .expect("valid function declaration regex")
    });
    static VARIABLE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)\b(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=")
            .expect("valid variable declaration regex")
    });
    static DEFAULT_FUNCTION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)\bexport\s+default\s+(?:async\s+)?function(?:\s+[A-Za-z_$][\w$]*)?\s*\(")
            .expect("valid default function regex")
    });
    static DEFAULT_SYMBOL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)\bexport\s+default\s+([A-Za-z_$][\w$]*)\s*;?")
            .expect("valid default symbol regex")
    });
    if symbol.is_empty()
        || !symbol.chars().all(|character| {
            character == '_' || character == '$' || character.is_ascii_alphanumeric()
        })
    {
        return None;
    }
    if symbol == "default" {
        if let Some(function) = DEFAULT_FUNCTION.find(source) {
            let open = source[function.end()..].find('{')? + function.end();
            let close = matching_delimiter(source, open, b'{', b'}')?;
            return Some(CodeRange {
                start: function.start(),
                end: close.saturating_add(1),
            });
        }
        if let Some(target) = DEFAULT_SYMBOL
            .captures(source)
            .and_then(|captures| captures.get(1))
        {
            if target.as_str() == "default" {
                return None;
            }
            return scope_range_for_symbol(source, target.as_str());
        }
    }
    if let Some(function) = FUNCTION.captures_iter(source).find_map(|captures| {
        if captures.get(1)?.as_str() == symbol {
            captures.get(0)
        } else {
            None
        }
    }) {
        let open = source[function.end()..].find('{')? + function.end();
        let close = matching_delimiter(source, open, b'{', b'}')?;
        return Some(CodeRange {
            start: function.start(),
            end: close.saturating_add(1),
        });
    }
    let variable = VARIABLE.captures_iter(source).find_map(|captures| {
        if captures.get(1)?.as_str() == symbol {
            captures.get(0)
        } else {
            None
        }
    })?;
    let tail = &source[variable.end()..];
    if let Some(arrow) = tail.find("=>") {
        let after_arrow = variable.end() + arrow + 2;
        let expression_start = source[after_arrow..]
            .find(|character: char| !character.is_whitespace())
            .map(|offset| after_arrow + offset);
        if let Some(open) = expression_start.filter(|offset| source.as_bytes()[*offset] == b'{')
            && let Some(close) = matching_delimiter(source, open, b'{', b'}')
        {
            return Some(CodeRange {
                start: variable.start(),
                end: close.saturating_add(1),
            });
        }
    }
    let end = source[variable.end()..]
        .find(';')
        .or_else(|| source[variable.end()..].find('\n'))
        .unwrap_or(tail.len())
        + variable.end();
    Some(CodeRange {
        start: variable.start(),
        end,
    })
}

fn matching_delimiter(source: &str, open: usize, opening: u8, closing: u8) -> Option<usize> {
    let bytes = source.as_bytes();
    if bytes.get(open).copied() != Some(opening) {
        return None;
    }
    let mut depth = 0_u32;
    let mut quote = None;
    let mut escaped = false;
    for (index, byte) in bytes.iter().copied().enumerate().skip(open) {
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
            value if value == opening => depth += 1,
            value if value == closing => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn stable_scope_key(value: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn apply_custom_rules(
    endpoint_id: &str,
    files: &[&JsFile],
    rules: &CompiledDependencyRules,
    matches_by_path: &BTreeMap<PathBuf, Vec<CustomRuleMatch>>,
    builder: &mut ApiMapBuilder,
) {
    let mut selected = BTreeMap::<usize, (&JsFile, &CustomRuleMatch)>::new();
    for file in files {
        let Some(matches) = matches_by_path.get(&file.path) else {
            continue;
        };
        for matched in matches {
            selected
                .entry(matched.rule_index)
                .and_modify(|current| {
                    if !current.1.package_match && matched.package_match {
                        *current = (*file, matched);
                    }
                })
                .or_insert((*file, matched));
        }
    }
    for (rule_index, (file, matched)) in selected {
        let Some(rule) = rules.rule(rule_index) else {
            continue;
        };
        let dependency_id = dependency_id(rule.kind, &rule.name);
        builder.add_dependency(Dependency {
            id: dependency_id.clone(),
            name: rule.name.clone(),
            kind: rule.kind,
            pinned: rule.pin,
            packages: rule.packages.clone(),
        });
        builder.add_relation(Relation {
            endpoint_id: endpoint_id.to_owned(),
            dependency_id,
            confidence: Confidence::Inferred,
            evidence: vec![Evidence {
                kind: EvidenceKind::Configuration,
                detail: matched.detail.clone(),
                source: Some(SourceRef {
                    file: file.relative.clone(),
                    line: matched.line,
                    column: 1,
                }),
            }],
        });
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use oxc_allocator::Allocator;
    use oxc_parser::Parser;
    use oxc_semantic::SemanticBuilder;
    use oxc_span::SourceType;

    use super::{
        SourceLocator, extract_syntax_facts, lexically_within_root, parse_import_locals,
        scope_for_symbol,
    };

    #[test]
    fn parses_common_import_bindings() {
        assert_eq!(parse_import_locals("PrismaClient"), ["PrismaClient"]);
        assert_eq!(
            parse_import_locals("{ Router, json as parseJson }"),
            ["Router", "parseJson"]
        );
        assert_eq!(parse_import_locals("* as stripe"), ["stripe"]);
    }

    #[test]
    fn isolates_route_handler_scopes() {
        let source = "export async function GET() { return loadUser(); }\nfunction unrelated() { stripe.charge(); }";
        let scope = scope_for_symbol(source, "GET").expect("GET scope");
        assert!(scope.contains("loadUser"));
        assert!(!scope.contains("stripe"));
    }

    #[test]
    fn rejects_recursive_default_export_fallbacks() {
        assert_eq!(scope_for_symbol("export default default;", "default"), None);
    }

    #[test]
    fn rejects_imports_that_lexically_escape_the_workspace() {
        let root = Path::new("/workspace");
        let importer = Path::new("/workspace/src/routes/users.ts");
        assert!(lexically_within_root(root, importer, "../../shared/users"));
        assert!(!lexically_within_root(root, importer, "../../../outside"));
        assert!(!lexically_within_root(root, importer, "@/../outside"));
    }

    #[test]
    fn uses_ast_ranges_for_multiline_typed_route_handlers() {
        let source = r"
export async function GET(
    _request: Request,
    context: { params: Promise<{ projectId: string }> },
): Promise<Response> {
    const { projectId } = await context.params;
    return loadProject(projectId);
}

function unrelated() {
    stripe.charge();
}
";
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();
        assert!(!parsed.panicked);
        assert!(parsed.diagnostics.is_empty());
        let semantic = SemanticBuilder::new()
            .with_build_nodes(true)
            .build(&parsed.program);
        assert!(semantic.diagnostics.is_empty());
        let locator = SourceLocator::new(source);
        let (_, _, symbols) = extract_syntax_facts(source, &semantic.semantic, &locator);
        let range = symbols.get("GET").expect("GET AST range");
        let scope = &source[range.start..range.end];

        assert!(scope.contains("loadProject"));
        assert!(!scope.contains("stripe"));
    }
}
