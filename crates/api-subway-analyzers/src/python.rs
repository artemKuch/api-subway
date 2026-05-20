use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
};

use api_subway_core::{
    ApiMapBuilder, Confidence, Dependency, Diagnostic, DiagnosticSeverity, Evidence, EvidenceKind,
    Relation, SourceRef, dependency_id,
};
use regex::Regex;
use tree_sitter::{Node, Parser};

use crate::{
    boundary, catalog,
    custom_rules::{CompiledDependencyRules, CustomRuleMatch, MAX_CUSTOM_RULE_MATCHES},
    discovery::relative_source_path,
    input::{ReadTextError, read_text_bounded},
    javascript::SourceLocator,
};

const MAX_SOURCE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_SYNTAX_DEPTH: usize = 128;
const MAX_CALLEE_DEPTH: usize = 64;
const MAX_ROUTE_GRAPH_SCOPES: usize = 10_000;
const MAX_ROUTE_GRAPH_QUEUE: usize = 20_000;

#[derive(Debug, Clone)]
pub(crate) struct PythonImportBinding {
    pub imported: String,
    pub local: String,
}

#[derive(Debug, Clone)]
pub(crate) struct PythonImport {
    pub package: String,
    pub locals: Vec<String>,
    pub bindings: Vec<PythonImportBinding>,
    pub resolved: Option<PathBuf>,
    pub line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CodeRange {
    start: usize,
    end: usize,
}

#[derive(Debug, Clone)]
struct PythonCallSite {
    path: Vec<String>,
    offset: usize,
    line: u32,
    column: u32,
    owner: Option<CodeRange>,
    dynamic_member: bool,
}

#[derive(Debug, Clone)]
struct PythonAlias {
    alias: String,
    target: String,
    offset: usize,
    owner: Option<CodeRange>,
}

#[derive(Debug, Clone)]
pub(crate) struct PythonFile {
    pub path: PathBuf,
    pub relative: String,
    pub source: String,
    pub imports: Vec<PythonImport>,
    pub parse_ok: bool,
    calls: Vec<PythonCallSite>,
    function_ranges: Vec<CodeRange>,
    aliases: Vec<PythonAlias>,
    syntax_truncated: bool,
}

#[derive(Debug)]
pub(crate) struct PythonIndex {
    files: BTreeMap<PathBuf, PythonFile>,
    diagnostics: Vec<Diagnostic>,
    custom_rules: Arc<CompiledDependencyRules>,
    custom_matches: BTreeMap<PathBuf, Vec<CustomRuleMatch>>,
}

impl PythonIndex {
    pub fn build(
        root: &Path,
        paths: &[PathBuf],
        custom_rules: Arc<CompiledDependencyRules>,
    ) -> Self {
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let mut index = Self {
            files: BTreeMap::new(),
            diagnostics: Vec::new(),
            custom_rules,
            custom_matches: BTreeMap::new(),
        };
        for path in paths {
            match index_file(&canonical_root, path) {
                Ok(file) => {
                    if file.syntax_truncated {
                        index.diagnostics.push(Diagnostic {
                            code: "python-syntax-depth".to_owned(),
                            severity: DiagnosticSeverity::Warning,
                            message: format!(
                                "Stopped traversing deeply nested syntax in {} after {MAX_SYNTAX_DEPTH} levels",
                                file.relative
                            ),
                            source: Some(SourceRef {
                                file: file.relative.clone(),
                                line: 1,
                                column: 1,
                            }),
                        });
                    }
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
                    .map(|import| (import.package.as_str(), import.line)),
                '.',
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

    pub fn files(&self) -> impl Iterator<Item = &PythonFile> {
        self.files.values()
    }

    pub fn get(&self, path: &Path) -> Option<&PythonFile> {
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
                let key = format!("<inline:{index}>");
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
                if let Some(entry) = catalog::by_package(&import.package) {
                    let dependency_id = dependency_id(entry.kind, entry.name);
                    builder.add_dependency(Dependency {
                        id: dependency_id.clone(),
                        name: entry.name.to_owned(),
                        kind: entry.kind,
                        pinned: false,
                        packages: vec![import.package.clone()],
                    });
                    for usage in usages {
                        let mut evidence = request.trace.clone();
                        evidence.push(Evidence {
                            kind: EvidenceKind::Call,
                            detail: format!(
                                "Resolved {}() call through the {} package",
                                usage.call.path.join("."),
                                import.package
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
                                    import.package, file.relative
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
            let mut local_calls = BTreeMap::<String, &PythonCallSite>::new();
            for call in calls
                .iter()
                .copied()
                .filter(|call| !call_is_import_bound(file, call))
            {
                if let Some(symbol) = call.path.first()
                    && scope_for_symbol(&file.source, symbol).is_some()
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
                code: "python-call-graph-budget".to_owned(),
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

fn forwarded_symbol(file: &PythonFile, symbol: &str) -> Option<(PathBuf, String, u32)> {
    let root = symbol.split('.').next().unwrap_or(symbol);
    file.imports.iter().find_map(|import| {
        let binding = import
            .bindings
            .iter()
            .find(|binding| binding.local == root)?;
        let resolved = import.resolved.clone()?;
        let target = if binding.imported == "*" {
            symbol
                .split_once('.')
                .map_or(root, |(_, member)| member)
                .to_owned()
        } else {
            binding.imported.clone()
        };
        Some((resolved, target, import.line))
    })
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

fn index_file(root: &Path, path: &Path) -> Result<PythonFile, Diagnostic> {
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
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("tree-sitter-python language is compatible");
    let tree = parser.parse(&source, None);
    let parse_ok = tree
        .as_ref()
        .is_some_and(|tree| !tree.root_node().has_error());
    let (calls, function_ranges, syntax_truncated) = tree.as_ref().map_or_else(
        || (Vec::new(), Vec::new(), false),
        |tree| extract_syntax_facts(tree.root_node(), &source),
    );
    let aliases = extract_aliases(&source, &function_ranges);
    let locator = SourceLocator::new(&source);
    let imports = extract_imports(root, path, &source, &locator);
    Ok(PythonFile {
        path: path.to_path_buf(),
        relative,
        source,
        imports,
        parse_ok,
        calls,
        function_ranges,
        aliases,
        syntax_truncated,
    })
}

fn extract_imports(
    root: &Path,
    path: &Path,
    source: &str,
    locator: &SourceLocator,
) -> Vec<PythonImport> {
    static FROM_IMPORT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^\s*from\s+([\.\w]+)\s+import\s+([^#\n]+)")
            .expect("valid Python import regex")
    });
    static IMPORT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^\s*import\s+([^#\n]+)").expect("valid Python import regex")
    });
    let mut imports = Vec::new();
    for captures in FROM_IMPORT.captures_iter(source) {
        let whole = captures.get(0).expect("whole import capture");
        let module = captures.get(1).map_or("", |value| value.as_str());
        let names = captures.get(2).map_or("", |value| value.as_str());
        for part in names.split(',') {
            let part = part
                .trim()
                .trim_matches(|character| character == '(' || character == ')');
            if part.is_empty() {
                continue;
            }
            let mut words = part.split_whitespace();
            let imported = words.next().unwrap_or_default();
            let local = words.nth(1).unwrap_or(imported);
            let resolved = resolve_python_module(root, path, module, Some(imported));
            imports.push(PythonImport {
                package: module.trim_start_matches('.').to_owned(),
                locals: vec![local.to_owned()],
                bindings: vec![PythonImportBinding {
                    imported: imported.to_owned(),
                    local: local.to_owned(),
                }],
                resolved,
                line: locator.line(whole.start()),
            });
        }
    }
    for captures in IMPORT.captures_iter(source) {
        let whole = captures.get(0).expect("whole import capture");
        let modules = captures.get(1).map_or("", |value| value.as_str());
        for part in modules.split(',') {
            let part = part.trim();
            let mut words = part.split_whitespace();
            let module = words.next().unwrap_or_default();
            let local = words
                .nth(1)
                .unwrap_or_else(|| module.split('.').next_back().unwrap_or(module));
            imports.push(PythonImport {
                package: module.to_owned(),
                locals: vec![local.to_owned()],
                bindings: vec![PythonImportBinding {
                    imported: "*".to_owned(),
                    local: local.to_owned(),
                }],
                resolved: resolve_python_module(root, path, module, None),
                line: locator.line(whole.start()),
            });
        }
    }
    imports.sort_by(|left, right| {
        left.line
            .cmp(&right.line)
            .then_with(|| left.package.cmp(&right.package))
    });
    imports
}

fn resolve_python_module(
    root: &Path,
    importer: &Path,
    module: &str,
    imported_name: Option<&str>,
) -> Option<PathBuf> {
    let dot_count = module
        .chars()
        .take_while(|character| *character == '.')
        .count();
    let module = module.trim_start_matches('.');
    let relative_base = if dot_count > 0 {
        let mut directory = importer.parent()?.to_path_buf();
        for _ in 1..dot_count {
            directory = directory.parent()?.to_path_buf();
        }
        Some(directory)
    } else {
        None
    };
    let mut candidates = Vec::new();
    let module_roots = relative_base.map_or_else(
        || {
            let mut roots = vec![root.to_path_buf()];
            let src = root.join("src");
            if src.is_dir() {
                roots.push(src);
            }
            roots
        },
        |base| vec![base],
    );
    for mut base in module_roots {
        if !module.is_empty() {
            base.extend(module.split('.'));
        }
        if let Some(name) = imported_name {
            candidates.push(base.join(format!("{name}.py")));
            candidates.push(base.join(name).join("__init__.py"));
        }
        candidates.push(base.with_extension("py"));
        candidates.push(base.join("__init__.py"));
    }
    candidates.into_iter().find_map(|candidate| {
        let canonical = candidate.canonicalize().ok()?;
        canonical.starts_with(root).then_some(canonical)
    })
}

fn extract_aliases(source: &str, function_ranges: &[CodeRange]) -> Vec<PythonAlias> {
    static ALIAS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^[ \t]*([A-Za-z_]\w*)\s*=\s*([A-Za-z_]\w*)\s*(?:\(|\.)")
            .expect("valid Python alias regex")
    });
    let mut aliases = ALIAS
        .captures_iter(source)
        .filter_map(|captures| {
            let whole = captures.get(0)?;
            Some(PythonAlias {
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
    call: &'a PythonCallSite,
    target_symbol: Option<String>,
}

fn extract_syntax_facts(
    root: Node<'_>,
    source: &str,
) -> (Vec<PythonCallSite>, Vec<CodeRange>, bool) {
    let mut calls = Vec::new();
    let mut function_ranges = Vec::new();
    let mut truncated = false;
    let mut stack = vec![(root, None, 0_usize)];
    while let Some((node, inherited_owner, depth)) = stack.pop() {
        if depth > MAX_SYNTAX_DEPTH {
            truncated = true;
            continue;
        }
        let range = CodeRange {
            start: node.start_byte(),
            end: node.end_byte(),
        };
        let owner = if matches!(node.kind(), "function_definition" | "lambda") {
            function_ranges.push(range);
            Some(range)
        } else {
            inherited_owner
        };
        if node.kind() == "call"
            && let Some(function) = node.child_by_field_name("function")
        {
            let mut dynamic_member = false;
            let path = python_callee_path(function, source, &mut dynamic_member, 0);
            if !path.is_empty() {
                let point = function.start_position();
                calls.push(PythonCallSite {
                    path,
                    offset: function.start_byte(),
                    line: u32::try_from(point.row.saturating_add(1)).unwrap_or(u32::MAX),
                    column: u32::try_from(point.column.saturating_add(1)).unwrap_or(u32::MAX),
                    owner,
                    dynamic_member,
                });
            }
        }
        let mut cursor = node.walk();
        stack.extend(
            node.children(&mut cursor)
                .map(|child| (child, owner, depth + 1)),
        );
    }
    calls.sort_by_key(|call| call.offset);
    calls.dedup_by(|left, right| left.offset == right.offset && left.path == right.path);
    function_ranges.sort_by_key(|range| (range.start, range.end));
    function_ranges.dedup();
    (calls, function_ranges, truncated)
}

fn python_callee_path(
    node: Node<'_>,
    source: &str,
    dynamic_member: &mut bool,
    depth: usize,
) -> Vec<String> {
    if depth >= MAX_CALLEE_DEPTH {
        *dynamic_member = true;
        return Vec::new();
    }
    match node.kind() {
        "identifier" => node
            .utf8_text(source.as_bytes())
            .ok()
            .map(|value| vec![value.to_owned()])
            .unwrap_or_default(),
        "attribute" => {
            let mut output = node
                .child_by_field_name("object")
                .map(|object| python_callee_path(object, source, dynamic_member, depth + 1))
                .unwrap_or_default();
            if let Some(attribute) = node
                .child_by_field_name("attribute")
                .and_then(|attribute| attribute.utf8_text(source.as_bytes()).ok())
            {
                output.push(attribute.to_owned());
            }
            output
        }
        "subscript" => {
            *dynamic_member = true;
            node.child_by_field_name("value")
                .map(|value| python_callee_path(value, source, dynamic_member, depth + 1))
                .unwrap_or_default()
        }
        "call" => node
            .child_by_field_name("function")
            .map(|function| python_callee_path(function, source, dynamic_member, depth + 1))
            .unwrap_or_default(),
        "parenthesized_expression" => {
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .next()
                .map(|child| python_callee_path(child, source, dynamic_member, depth + 1))
                .unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

fn scope_region_for_symbol(file: &PythonFile, symbol: &str) -> Option<ScopeRegion> {
    let range = scope_range_for_symbol(&file.source, symbol)?;
    Some(ScopeRegion {
        range,
        owner: outermost_function_within(&file.function_ranges, range),
    })
}

fn scope_region_for_inline(file: &PythonFile, inline: &str) -> Option<ScopeRegion> {
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

fn calls_in_scopes<'a>(file: &'a PythonFile, scopes: &[ScopeRegion]) -> Vec<&'a PythonCallSite> {
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
    file: &PythonFile,
    import: &PythonImport,
    calls: &[&'a PythonCallSite],
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
            } else if binding.imported == "*" {
                call.path.get(1).cloned()
            } else {
                Some(binding.imported.clone())
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

fn call_source(file: &PythonFile, call: &PythonCallSite) -> SourceRef {
    SourceRef {
        file: file.relative.clone(),
        line: call.line,
        column: call.column,
    }
}

fn call_is_import_bound(file: &PythonFile, call: &PythonCallSite) -> bool {
    file.imports
        .iter()
        .flat_map(|import| &import.bindings)
        .any(|binding| call_uses_binding(file, binding, call))
}

fn call_uses_binding(
    file: &PythonFile,
    binding: &PythonImportBinding,
    call: &PythonCallSite,
) -> bool {
    let Some(root) = call.path.first() else {
        return false;
    };
    if root == &binding.local {
        return resolved_alias_target(file, root, call).is_none();
    }
    resolved_alias_target(file, root, call).is_some_and(|target| target == binding.local)
}

fn resolved_alias_target<'a>(
    file: &'a PythonFile,
    alias: &str,
    call: &PythonCallSite,
) -> Option<&'a str> {
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

fn alias_is_visible(alias: &PythonAlias, call: &PythonCallSite) -> bool {
    if alias.offset >= call.offset {
        return false;
    }
    alias.owner.is_none_or(|owner| {
        call.owner.is_some_and(|call_owner| {
            call_owner.start >= owner.start && call_owner.end <= owner.end
        })
    })
}

fn alias_scope_size(alias: &PythonAlias) -> usize {
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

fn scope_for_symbol(source: &str, symbol: &str) -> Option<String> {
    let range = scope_range_for_symbol(source, symbol)?;
    source.get(range.start..range.end).map(str::to_owned)
}

fn scope_range_for_symbol(source: &str, symbol: &str) -> Option<CodeRange> {
    if symbol.is_empty()
        || !symbol
            .chars()
            .all(|character| character == '_' || character.is_ascii_alphanumeric())
    {
        return None;
    }
    let definition = Regex::new(&format!(
        r"(?m)^([ \t]*)(?:async\s+)?def\s+{}\s*\(",
        regex::escape(symbol)
    ))
    .ok()?
    .find(source)?;
    let line_start = source[..definition.start()]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    let definition_line = source.get(line_start..)?.lines().next()?;
    let indent = definition_line
        .chars()
        .take_while(|character| character.is_whitespace())
        .count();
    let mut end = source.len();
    let mut offset = line_start + definition_line.len() + 1;
    for line in source.get(offset..)?.split_inclusive('\n') {
        let trimmed = line.trim();
        let next_indent = line
            .chars()
            .take_while(|character| character.is_whitespace())
            .count();
        if !trimmed.is_empty() && !trimmed.starts_with('#') && next_indent <= indent {
            end = offset;
            break;
        }
        offset += line.len();
    }
    Some(CodeRange {
        start: line_start,
        end,
    })
}

fn apply_custom_rules(
    endpoint_id: &str,
    files: &[&PythonFile],
    rules: &CompiledDependencyRules,
    matches_by_path: &BTreeMap<PathBuf, Vec<CustomRuleMatch>>,
    builder: &mut ApiMapBuilder,
) {
    let mut selected = BTreeMap::<usize, (&PythonFile, &CustomRuleMatch)>::new();
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

    use super::{resolve_python_module, scope_for_symbol};

    #[test]
    fn unresolved_modules_do_not_escape_the_workspace() {
        assert!(
            resolve_python_module(
                Path::new("/tmp/absent"),
                Path::new("/tmp/absent/app.py"),
                "os",
                None
            )
            .is_none()
        );
    }

    #[test]
    fn isolates_python_handler_scopes() {
        let source =
            "def route():\n    return query()\n\ndef unrelated():\n    stripe.Customer.list()\n";
        let scope = scope_for_symbol(source, "route").expect("route scope");
        assert!(scope.contains("query"));
        assert!(!scope.contains("stripe"));
    }
}
