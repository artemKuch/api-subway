use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

use api_subway_core::{
    ApiMapBuilder, ApiMapV1, ApiSubwayConfig, Confidence, Dependency, DependencyKind, Diagnostic,
    DiagnosticSeverity, Endpoint, Evidence, Relation,
};
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use thiserror::Error;

use crate::{
    contracts, custom_rules::CompiledDependencyRules, express, fastapi, javascript::JsIndex, next,
    openapi, python::PythonIndex,
};

use crate::input::{ReadTextError, read_text_bounded};

const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_OPENAPI_FILES: usize = 64;
const MAX_SOURCE_FILES: usize = 100_000;
const MAX_TOTAL_SOURCE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_MODEL_DIAGNOSTICS: usize = 50_000;
const MAX_PROJECT_NAME_LENGTH: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Framework {
    Auto,
    Next,
    Express,
    FastApi,
}

impl FromStr for Framework {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "next" | "nextjs" => Ok(Self::Next),
            "express" => Ok(Self::Express),
            "fastapi" | "fast-api" => Ok(Self::FastApi),
            _ => Err(format!("unsupported framework '{value}'")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AnalyzeOptions {
    pub root: PathBuf,
    pub frameworks: Vec<Framework>,
    pub openapi: Vec<PathBuf>,
    pub config: ApiSubwayConfig,
}

impl AnalyzeOptions {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            frameworks: vec![Framework::Auto],
            openapi: Vec::new(),
            config: ApiSubwayConfig::default(),
        }
    }
}

#[derive(Debug, Error)]
pub enum AnalyzerError {
    #[error("analysis root does not exist: {0}")]
    MissingRoot(PathBuf),
    #[error("failed to resolve analysis root {path}: {source}")]
    Root {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("analysis root must be a directory: {0}")]
    RootNotDirectory(PathBuf),
    #[error("failed to read OpenAPI document {path}: {source}")]
    OpenApiRead {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("OpenAPI document exceeds the 16 MiB input budget: {0}")]
    OpenApiBudget(PathBuf),
    #[error("failed to parse OpenAPI document {path}: {message}")]
    OpenApiParse { path: PathBuf, message: String },
    #[error("OpenAPI document resolves outside the analysis root: {0}")]
    OpenApiOutsideRoot(PathBuf),
    #[error("OpenAPI document must be a regular file: {0}")]
    OpenApiNotFile(PathBuf),
    #[error("no more than 64 OpenAPI documents can be analyzed at once (received {0})")]
    OpenApiCount(usize),
    #[error("invalid {scope} glob '{pattern}': {message}")]
    InvalidGlob {
        scope: &'static str,
        pattern: String,
        message: String,
    },
    #[error(
        "workspace source budget exceeded at {files} files and {bytes} bytes; limits are 100000 files and 512 MiB (use scan.exclude to narrow the root)"
    )]
    WorkspaceBudget { files: usize, bytes: u64 },
    #[error("generated map exceeds a safety budget: {0}")]
    ModelBudget(String),
}

#[derive(Debug, Clone)]
pub(crate) struct RouteRecord {
    pub endpoint: Endpoint,
    pub source_path: PathBuf,
    pub entry_symbols: Vec<String>,
    pub inline_code: Vec<String>,
    pub dependencies: Vec<ExplicitDependency>,
}

#[derive(Debug, Clone)]
pub(crate) struct ExplicitDependency {
    pub name: String,
    pub kind: DependencyKind,
    pub confidence: Confidence,
    pub evidence: Evidence,
    pub pinned: bool,
    pub packages: Vec<String>,
}

pub(crate) struct AdapterOutput {
    pub routes: Vec<RouteRecord>,
    pub diagnostics: Vec<Diagnostic>,
}

struct DiscoveredSources {
    javascript: Vec<PathBuf>,
    python: Vec<PathBuf>,
    diagnostics: Vec<Diagnostic>,
}

impl AdapterOutput {
    pub fn empty() -> Self {
        Self {
            routes: Vec::new(),
            diagnostics: Vec::new(),
        }
    }
}

pub fn analyze(options: &AnalyzeOptions) -> Result<ApiMapV1, AnalyzerError> {
    if !options.root.exists() {
        return Err(AnalyzerError::MissingRoot(options.root.clone()));
    }
    let root = options
        .root
        .canonicalize()
        .map_err(|source| AnalyzerError::Root {
            path: options.root.clone(),
            source,
        })?;
    if !root.is_dir() {
        return Err(AnalyzerError::RootNotDirectory(root));
    }
    validate_config_globs(&options.config)?;
    let mut openapi_paths = options.config.openapi.clone();
    openapi_paths.extend(options.openapi.iter().cloned());
    openapi_paths.sort();
    openapi_paths.dedup();
    if openapi_paths.len() > MAX_OPENAPI_FILES {
        return Err(AnalyzerError::OpenApiCount(openapi_paths.len()));
    }
    let custom_rules = Arc::new(CompiledDependencyRules::new(&options.config).map_err(
        |message| AnalyzerError::InvalidGlob {
            scope: "dependency.path_globs",
            pattern: "<compiled rules>".to_owned(),
            message,
        },
    )?);
    let manifests = read_manifests(&root);
    let (project_name, project_name_truncated) =
        discover_project_name(&root, &manifests.package_json);
    let sources = discover_sources(&root, &options.config)?;
    let js_index = JsIndex::build(&root, &sources.javascript, Arc::clone(&custom_rules));
    let python_index = PythonIndex::build(&root, &sources.python, Arc::clone(&custom_rules));
    let frameworks = resolve_frameworks(
        options,
        &js_index,
        &python_index,
        &manifests.package_json,
        &manifests.python,
    );
    let mut builder = ApiMapBuilder::new(project_name);
    if project_name_truncated {
        builder.add_diagnostic(Diagnostic {
            code: "project-name-truncated".to_owned(),
            severity: DiagnosticSeverity::Warning,
            message: format!(
                "Project name was truncated to the supported {MAX_PROJECT_NAME_LENGTH}-character limit"
            ),
            source: Some(api_subway_core::SourceRef {
                file: "package.json".to_owned(),
                line: 1,
                column: 1,
            }),
        });
    }
    for diagnostic in manifests
        .diagnostics
        .iter()
        .chain(sources.diagnostics.iter())
        .chain(js_index.diagnostics())
        .chain(python_index.diagnostics())
    {
        builder.add_diagnostic(diagnostic.clone());
    }

    let mut outputs = Vec::new();
    if frameworks.contains(&Framework::Next) {
        builder.add_framework("next");
        outputs.push(next::analyze(&root, &js_index));
    }
    if frameworks.contains(&Framework::Express) {
        builder.add_framework("express");
        outputs.push(express::analyze(&root, &js_index));
    }
    if frameworks.contains(&Framework::FastApi) {
        builder.add_framework("fastapi");
        outputs.push(fastapi::analyze(&root, &python_index));
    }

    let include_routes = build_glob_set(&options.config.map.include_routes);
    let exclude_routes = build_glob_set(&options.config.map.exclude_routes);
    let mut seen_routes = BTreeSet::new();
    for output in outputs {
        for diagnostic in output.diagnostics {
            builder.add_diagnostic(diagnostic);
        }
        for mut route in output.routes {
            if !route_is_included(
                &route.endpoint.path,
                include_routes.as_ref(),
                exclude_routes.as_ref(),
            ) {
                continue;
            }
            if !seen_routes.insert(route.endpoint.id.clone()) {
                builder.add_diagnostic(Diagnostic {
                    code: "duplicate-route".to_owned(),
                    severity: DiagnosticSeverity::Warning,
                    message: format!(
                        "Merged duplicate declaration for {} {}",
                        route.endpoint.method, route.endpoint.path
                    ),
                    source: route.endpoint.sources.first().cloned(),
                });
            }
            let endpoint_id = route.endpoint.id.clone();
            let framework = route.endpoint.framework.clone();
            let analysis = if framework == "fastapi" {
                contracts::python::analyze_route(&route, &python_index)
            } else {
                contracts::zod::analyze_route(&route, &js_index)
            };
            route.endpoint.contract = analysis.contract;
            for schema in analysis.schemas {
                builder.add_schema(schema);
            }
            for diagnostic in analysis.diagnostics {
                builder.add_diagnostic(diagnostic);
            }
            let mut path_schemas = Vec::new();
            contracts::add_inferred_path_parameters(&mut route.endpoint, &mut path_schemas);
            for schema in path_schemas {
                builder.add_schema(schema);
            }
            builder.add_endpoint(route.endpoint);
            for explicit in route.dependencies {
                let dependency_id = api_subway_core::dependency_id(explicit.kind, &explicit.name);
                builder.add_dependency(Dependency {
                    id: dependency_id.clone(),
                    name: explicit.name,
                    kind: explicit.kind,
                    pinned: explicit.pinned,
                    packages: explicit.packages,
                });
                builder.add_relation(Relation {
                    endpoint_id: endpoint_id.clone(),
                    dependency_id,
                    confidence: explicit.confidence,
                    evidence: vec![explicit.evidence],
                });
            }
            if framework == "fastapi" {
                python_index.classify_route(
                    &endpoint_id,
                    &route.source_path,
                    &route.entry_symbols,
                    &route.inline_code,
                    &mut builder,
                );
            } else {
                js_index.classify_route(
                    &endpoint_id,
                    &route.source_path,
                    &route.entry_symbols,
                    &route.inline_code,
                    &mut builder,
                );
            }
        }
    }

    for path in openapi_paths {
        let path = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
        openapi::merge(&root, &path, &mut builder)?;
        builder.add_framework("openapi");
    }

    let mut map = builder.build();
    if map.endpoints.is_empty() {
        map.diagnostics
            .truncate(MAX_MODEL_DIAGNOSTICS.saturating_sub(1));
        map.diagnostics.push(Diagnostic {
            code: "no-routes".to_owned(),
            severity: DiagnosticSeverity::Warning,
            message: "No supported API routes were found; verify the root, framework selection, and scan exclusions"
                .to_owned(),
            source: None,
        });
        map.diagnostics.sort();
        map.diagnostics.dedup();
    }
    crate::model_budget::validate(&map).map_err(AnalyzerError::ModelBudget)?;
    Ok(map)
}

fn validate_config_globs(config: &ApiSubwayConfig) -> Result<(), AnalyzerError> {
    let groups = [
        ("scan.exclude", config.scan.exclude.as_slice()),
        ("map.include_routes", config.map.include_routes.as_slice()),
        ("map.exclude_routes", config.map.exclude_routes.as_slice()),
    ];
    for (scope, patterns) in groups {
        for pattern in patterns {
            Glob::new(pattern).map_err(|error| AnalyzerError::InvalidGlob {
                scope,
                pattern: pattern.clone(),
                message: error.to_string(),
            })?;
        }
    }
    for dependency in &config.dependencies {
        for pattern in &dependency.path_globs {
            Glob::new(pattern).map_err(|error| AnalyzerError::InvalidGlob {
                scope: "dependency.path_globs",
                pattern: pattern.clone(),
                message: error.to_string(),
            })?;
        }
    }
    Ok(())
}

fn discover_sources(
    root: &Path,
    config: &ApiSubwayConfig,
) -> Result<DiscoveredSources, AnalyzerError> {
    let exclude_set = build_glob_set(&config.scan.exclude);
    let mut javascript = Vec::new();
    let mut python = Vec::new();
    let mut diagnostics = Vec::new();
    let mut total_source_bytes = 0_u64;
    let walker = WalkBuilder::new(root)
        .follow_links(false)
        .hidden(false)
        .standard_filters(true)
        .build();
    for result in walker {
        let entry = match result {
            Ok(entry) => entry,
            Err(error) => {
                let source = discovery_error_path(&error).and_then(|path| {
                    relative_source_path(root, path).map(|file| api_subway_core::SourceRef {
                        file,
                        line: 1,
                        column: 1,
                    })
                });
                let reason = error.io_error().map_or_else(
                    || "filesystem traversal failed".to_owned(),
                    |error| format!("filesystem traversal failed: {}", error.kind()),
                );
                diagnostics.push(Diagnostic {
                    code: "source-discovery".to_owned(),
                    severity: DiagnosticSeverity::Warning,
                    message: reason,
                    source,
                });
                continue;
            }
        };
        let path = entry.path();
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            continue;
        }
        let relative = relative_source_path(root, path).unwrap_or_default();
        if has_ignored_component(path)
            || exclude_set
                .as_ref()
                .is_some_and(|set| set.is_match(&relative))
        {
            continue;
        }
        let language = match path.extension().and_then(|extension| extension.to_str()) {
            Some("js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs") => "javascript",
            Some("py") => "python",
            _ => continue,
        };
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                let reason = error.io_error().map_or_else(
                    || "metadata lookup failed".to_owned(),
                    |error| format!("metadata lookup failed: {}", error.kind()),
                );
                diagnostics.push(Diagnostic {
                    code: "source-discovery".to_owned(),
                    severity: DiagnosticSeverity::Warning,
                    message: reason,
                    source: Some(api_subway_core::SourceRef {
                        file: relative,
                        line: 1,
                        column: 1,
                    }),
                });
                continue;
            }
        };
        let source_files = javascript.len() + python.len() + 1;
        let source_bytes = total_source_bytes.saturating_add(metadata.len());
        ensure_workspace_budget(source_files, source_bytes)?;
        total_source_bytes = source_bytes;
        if language == "javascript" {
            javascript.push(path.to_path_buf());
        } else {
            python.push(path.to_path_buf());
        }
    }
    javascript.sort();
    python.sort();
    diagnostics.sort();
    diagnostics.dedup();
    Ok(DiscoveredSources {
        javascript,
        python,
        diagnostics,
    })
}

fn ensure_workspace_budget(files: usize, bytes: u64) -> Result<(), AnalyzerError> {
    if files > MAX_SOURCE_FILES || bytes > MAX_TOTAL_SOURCE_BYTES {
        return Err(AnalyzerError::WorkspaceBudget { files, bytes });
    }
    Ok(())
}

fn discovery_error_path(error: &ignore::Error) -> Option<&Path> {
    match error {
        ignore::Error::Partial(errors) => errors.iter().find_map(discovery_error_path),
        ignore::Error::WithLineNumber { err, .. } | ignore::Error::WithDepth { err, .. } => {
            discovery_error_path(err)
        }
        ignore::Error::WithPath { path, .. } => Some(path),
        ignore::Error::Loop { child, .. } => Some(child),
        ignore::Error::Io(_)
        | ignore::Error::Glob { .. }
        | ignore::Error::UnrecognizedFileType(_)
        | ignore::Error::InvalidDefinition => None,
    }
}

fn has_ignored_component(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component.as_os_str().to_str(),
            Some("node_modules" | "target" | ".git" | ".venv" | "venv" | "__pycache__")
        )
    })
}

fn resolve_frameworks(
    options: &AnalyzeOptions,
    js_index: &JsIndex,
    python_index: &PythonIndex,
    package_json: &str,
    python_manifest: &str,
) -> BTreeSet<Framework> {
    let mut requested = options.frameworks.iter().copied().collect::<BTreeSet<_>>();
    if requested.is_empty() {
        requested.insert(Framework::Auto);
    }
    if !requested.remove(&Framework::Auto) {
        return requested;
    }
    if package_json.contains("\"next\"")
        || js_index
            .files()
            .any(|file| next::is_route_file(&file.relative))
    {
        requested.insert(Framework::Next);
    }
    if package_json.contains("\"express\"")
        || js_index
            .files()
            .any(|file| file.source.contains("express()") || file.source.contains("express.Router"))
    {
        requested.insert(Framework::Express);
    }
    if python_manifest.to_ascii_lowercase().contains("fastapi")
        || python_index
            .files()
            .any(|file| file.source.contains("FastAPI(") || file.source.contains("APIRouter("))
    {
        requested.insert(Framework::FastApi);
    }
    requested
}

fn discover_project_name(root: &Path, package_json: &str) -> (String, bool) {
    let package_name = serde_json::from_str::<serde_json::Value>(package_json)
        .ok()
        .and_then(|value| value.get("name")?.as_str().map(str::to_owned))
        .filter(|name| !name.trim().is_empty());
    let discovered = package_name.unwrap_or_else(|| {
        root.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("api")
            .to_owned()
    });
    let truncated = discovered.chars().count() > MAX_PROJECT_NAME_LENGTH;
    (
        discovered.chars().take(MAX_PROJECT_NAME_LENGTH).collect(),
        truncated,
    )
}

struct ManifestInputs {
    package_json: String,
    python: String,
    diagnostics: Vec<Diagnostic>,
}

fn read_manifests(root: &Path) -> ManifestInputs {
    let mut diagnostics = Vec::new();
    let package_json = read_optional_manifest(root, "package.json", &mut diagnostics);
    if !package_json.is_empty()
        && let Err(error) = serde_json::from_str::<serde_json::Value>(&package_json)
    {
        diagnostics.push(Diagnostic {
            code: "manifest-parse".to_owned(),
            severity: DiagnosticSeverity::Warning,
            message: format!("Could not parse package.json: {error}"),
            source: Some(api_subway_core::SourceRef {
                file: "package.json".to_owned(),
                line: 1,
                column: 1,
            }),
        });
    }
    let pyproject = read_optional_manifest(root, "pyproject.toml", &mut diagnostics);
    let requirements = read_optional_manifest(root, "requirements.txt", &mut diagnostics);
    diagnostics.sort();
    diagnostics.dedup();
    ManifestInputs {
        package_json,
        python: format!("{pyproject}\n{requirements}"),
        diagnostics,
    }
}

fn read_optional_manifest(root: &Path, name: &str, diagnostics: &mut Vec<Diagnostic>) -> String {
    let path = root.join(name);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return String::new(),
        Err(error) => {
            diagnostics.push(manifest_diagnostic(
                "manifest-read",
                name,
                format!("Could not inspect {name}: {}", error.kind()),
            ));
            return String::new();
        }
        Ok(metadata) if !metadata.file_type().is_file() => {
            diagnostics.push(manifest_diagnostic(
                "manifest-unsafe",
                name,
                format!(
                    "Skipped {name}: manifests must be regular files, not links or directories"
                ),
            ));
            return String::new();
        }
        Ok(_) => {}
    }
    match read_text_bounded(&path, MAX_MANIFEST_BYTES) {
        Ok(contents) => contents,
        Err(ReadTextError::Budget) => {
            diagnostics.push(Diagnostic {
                code: "manifest-budget".to_owned(),
                severity: DiagnosticSeverity::Warning,
                message: format!("Skipped {name}: manifest exceeds the 1 MiB budget"),
                source: Some(api_subway_core::SourceRef {
                    file: name.to_owned(),
                    line: 1,
                    column: 1,
                }),
            });
            String::new()
        }
        Err(ReadTextError::Io(error)) => {
            diagnostics.push(Diagnostic {
                code: "manifest-read".to_owned(),
                severity: DiagnosticSeverity::Warning,
                message: format!("Could not read {name}: {}", error.kind()),
                source: Some(api_subway_core::SourceRef {
                    file: name.to_owned(),
                    line: 1,
                    column: 1,
                }),
            });
            String::new()
        }
    }
}

fn manifest_diagnostic(code: &str, name: &str, message: String) -> Diagnostic {
    Diagnostic {
        code: code.to_owned(),
        severity: DiagnosticSeverity::Warning,
        message,
        source: Some(api_subway_core::SourceRef {
            file: name.to_owned(),
            line: 1,
            column: 1,
        }),
    }
}

fn route_is_included(path: &str, include: Option<&GlobSet>, exclude: Option<&GlobSet>) -> bool {
    include.is_none_or(|set| set.is_match(path)) && !exclude.is_some_and(|set| set.is_match(path))
}

fn build_glob_set(patterns: &[String]) -> Option<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    let mut added = false;
    for pattern in patterns {
        if let Ok(glob) = Glob::new(pattern) {
            builder.add(glob);
            added = true;
        }
    }
    added.then(|| builder.build().ok()).flatten()
}

pub(crate) fn relative_source_path(root: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(root)
        .ok()
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        AnalyzeOptions, AnalyzerError, Framework, MAX_OPENAPI_FILES, MAX_PROJECT_NAME_LENGTH,
        MAX_SOURCE_FILES, MAX_TOTAL_SOURCE_BYTES, analyze, discover_project_name,
        ensure_workspace_budget,
    };

    #[test]
    fn workspace_budget_accepts_the_boundary_and_rejects_overflow() {
        assert!(ensure_workspace_budget(MAX_SOURCE_FILES, MAX_TOTAL_SOURCE_BYTES).is_ok());
        assert!(matches!(
            ensure_workspace_budget(MAX_SOURCE_FILES + 1, 0),
            Err(AnalyzerError::WorkspaceBudget { .. })
        ));
        assert!(matches!(
            ensure_workspace_budget(1, MAX_TOTAL_SOURCE_BYTES + 1),
            Err(AnalyzerError::WorkspaceBudget { .. })
        ));
    }

    #[test]
    fn project_names_are_non_empty_and_bounded() {
        let root = std::path::Path::new("/workspace/fallback");
        assert_eq!(
            discover_project_name(root, r#"{"name":""}"#),
            ("fallback".to_owned(), false)
        );

        let name = "é".repeat(MAX_PROJECT_NAME_LENGTH + 1);
        let manifest = format!(r#"{{"name":"{name}"}}"#);
        let (bounded, truncated) = discover_project_name(root, &manifest);
        assert!(truncated);
        assert_eq!(bounded.chars().count(), MAX_PROJECT_NAME_LENGTH);
    }

    #[test]
    fn rejects_non_directory_roots_and_excess_openapi_inputs() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let file = directory.path().join("root.ts");
        fs::write(&file, "export const value = true;\n").expect("source file");
        let canonical_file = file.canonicalize().expect("canonical source file");
        assert!(matches!(
            analyze(&AnalyzeOptions::new(&file)),
            Err(AnalyzerError::RootNotDirectory(path)) if path == canonical_file
        ));

        let mut options = AnalyzeOptions::new(directory.path());
        options.frameworks = vec![Framework::Express];
        options.openapi = (0..=MAX_OPENAPI_FILES)
            .map(|index| format!("spec-{index}.json").into())
            .collect();
        assert!(matches!(
            analyze(&options),
            Err(AnalyzerError::OpenApiCount(count)) if count == MAX_OPENAPI_FILES + 1
        ));
    }
}
