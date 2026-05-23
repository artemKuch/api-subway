use std::{path::Path, sync::LazyLock};

use api_subway_core::{
    Confidence, DependencyKind, Diagnostic, DiagnosticSeverity, Endpoint, Evidence, EvidenceKind,
    SourceRef, canonical_endpoint_id, district_for_path, normalize_route_path,
};
use regex::Regex;

use crate::{
    discovery::{AdapterOutput, ExplicitDependency, RouteRecord},
    javascript::{JsIndex, SourceLocator},
};

const MAX_PROXY_MATCHERS: usize = 1_000;

pub(crate) fn is_route_file(relative: &str) -> bool {
    let normalized = relative.replace('\\', "/");
    let Some(file_name) = normalized.rsplit('/').next() else {
        return false;
    };
    matches!(
        file_name,
        "route.js" | "route.jsx" | "route.ts" | "route.tsx"
    ) && (normalized.starts_with("app/") || normalized.starts_with("src/app/"))
}

pub(crate) fn analyze(_root: &Path, index: &JsIndex) -> AdapterOutput {
    let mut output = AdapterOutput::empty();
    for file in index.files().filter(|file| is_route_file(&file.relative)) {
        let locator = SourceLocator::new(&file.source);
        if !file.parse_ok {
            output.diagnostics.push(Diagnostic {
                code: "next-parse".to_owned(),
                severity: DiagnosticSeverity::Warning,
                message: format!(
                    "Oxc reported syntax errors in {}; route exports were recovered conservatively",
                    file.relative
                ),
                source: Some(SourceRef {
                    file: file.relative.clone(),
                    line: 1,
                    column: 1,
                }),
            });
        }
        let route_path = route_path_from_file(&file.relative);
        let exports = extract_http_exports(&file.source);
        if exports.is_empty() {
            output.diagnostics.push(Diagnostic {
                code: "next-no-method".to_owned(),
                severity: DiagnosticSeverity::Warning,
                message: format!("No static HTTP method export found in {}", file.relative),
                source: Some(SourceRef {
                    file: file.relative.clone(),
                    line: 1,
                    column: 1,
                }),
            });
        }
        for (method, offset) in exports {
            let path = normalize_route_path(&route_path);
            output.routes.push(RouteRecord {
                endpoint: Endpoint {
                    id: canonical_endpoint_id(&method, &path),
                    method: method.clone(),
                    path: path.clone(),
                    display_path: path.clone(),
                    district: district_for_path(&path),
                    framework: "next".to_owned(),
                    operation_id: None,
                    tags: Vec::new(),
                    sources: vec![SourceRef {
                        file: file.relative.clone(),
                        line: locator.line(offset),
                        column: 1,
                    }],
                    spec_only: false,
                    contract: None,
                },
                source_path: file.path.clone(),
                entry_symbols: vec![method.clone()],
                inline_code: Vec::new(),
                dependencies: Vec::new(),
            });
        }
    }
    apply_proxy(index, &mut output);
    output.routes.sort_by(|left, right| {
        left.endpoint
            .path
            .cmp(&right.endpoint.path)
            .then_with(|| left.endpoint.method.cmp(&right.endpoint.method))
    });
    output
}

fn route_path_from_file(relative: &str) -> String {
    let normalized = relative.replace('\\', "/");
    let marker = if normalized.starts_with("src/app/") {
        "src/app/"
    } else {
        "app/"
    };
    let directory = normalized
        .strip_prefix(marker)
        .unwrap_or(&normalized)
        .rsplit_once('/')
        .map_or("", |(directory, _)| directory);
    normalize_route_path(directory)
}

fn extract_http_exports(source: &str) -> Vec<(String, usize)> {
    static DECLARATION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?m)\bexport\s+(?:async\s+)?(?:function|const|let|var)\s+(GET|POST|PUT|PATCH|DELETE|OPTIONS|HEAD)\b",
        )
        .expect("valid Next.js export regex")
    });
    static REEXPORT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?m)\bexport\s*\{[^}]*\bas\s+(GET|POST|PUT|PATCH|DELETE|OPTIONS|HEAD)\b[^}]*\}",
        )
        .expect("valid Next.js re-export regex")
    });
    let mut exports = DECLARATION
        .captures_iter(source)
        .filter_map(|captures| {
            Some((
                captures.get(1)?.as_str().to_owned(),
                captures.get(0)?.start(),
            ))
        })
        .chain(REEXPORT.captures_iter(source).filter_map(|captures| {
            Some((
                captures.get(1)?.as_str().to_owned(),
                captures.get(0)?.start(),
            ))
        }))
        .collect::<Vec<_>>();
    exports.sort();
    exports.dedup_by(|left, right| left.0 == right.0);
    exports
}

fn apply_proxy(index: &JsIndex, output: &mut AdapterOutput) {
    for file in index.files().filter(|file| is_proxy_file(&file.relative)) {
        let mut matchers = extract_matchers(&file.source);
        if matchers.len() > MAX_PROXY_MATCHERS {
            matchers.truncate(MAX_PROXY_MATCHERS);
            output.diagnostics.push(Diagnostic {
                code: "next-matcher-budget".to_owned(),
                severity: DiagnosticSeverity::Warning,
                message: format!(
                    "Only the first {MAX_PROXY_MATCHERS} constant matchers in {} were analyzed",
                    file.relative
                ),
                source: Some(SourceRef {
                    file: file.relative.clone(),
                    line: 1,
                    column: 1,
                }),
            });
        }
        let dynamic_matcher = file.source.contains("matcher") && matchers.is_empty();
        if dynamic_matcher {
            output.diagnostics.push(Diagnostic {
                code: "next-dynamic-matcher".to_owned(),
                severity: DiagnosticSeverity::Warning,
                message: format!("Could not resolve a constant matcher in {}", file.relative),
                source: Some(SourceRef {
                    file: file.relative.clone(),
                    line: 1,
                    column: 1,
                }),
            });
            continue;
        }
        let name = if file.relative.ends_with("proxy.ts") || file.relative.ends_with("proxy.js") {
            "Next Proxy"
        } else {
            "Next Middleware"
        };
        for route in &mut output.routes {
            if matchers.is_empty()
                || matchers
                    .iter()
                    .any(|matcher| matcher_matches(matcher, &route.endpoint.path))
            {
                route.dependencies.push(ExplicitDependency {
                    name: name.to_owned(),
                    kind: DependencyKind::Middleware,
                    confidence: Confidence::Exact,
                    evidence: Evidence {
                        kind: EvidenceKind::Framework,
                        detail: format!("Matched by {name} configuration"),
                        source: Some(SourceRef {
                            file: file.relative.clone(),
                            line: 1,
                            column: 1,
                        }),
                    },
                    pinned: false,
                    packages: Vec::new(),
                });
            }
        }
    }
}

fn is_proxy_file(relative: &str) -> bool {
    matches!(
        relative,
        "proxy.ts"
            | "proxy.js"
            | "src/proxy.ts"
            | "src/proxy.js"
            | "middleware.ts"
            | "middleware.js"
            | "src/middleware.ts"
            | "src/middleware.js"
    )
}

fn extract_matchers(source: &str) -> Vec<String> {
    static MATCHER_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)matcher\s*:\s*(\[[^\]]*\]|['\"][^'\"]+['\"])"#)
            .expect("valid matcher regex")
    });
    static STRING: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"[\"']([^\"']+)[\"']"#).expect("valid string regex"));
    let mut matchers = MATCHER_BLOCK
        .captures(source)
        .and_then(|captures| captures.get(1))
        .map(|block| {
            STRING
                .captures_iter(block.as_str())
                .filter_map(|captures| captures.get(1).map(|value| value.as_str().to_owned()))
                .take(MAX_PROXY_MATCHERS + 1)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    matchers.sort();
    matchers.dedup();
    matchers
}

fn matcher_matches(matcher: &str, path: &str) -> bool {
    let static_prefix = matcher
        .split([':', '(', '*'])
        .next()
        .unwrap_or(matcher)
        .trim_end_matches('/');
    static_prefix.is_empty()
        || path == static_prefix
        || path
            .strip_prefix(static_prefix)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::{extract_http_exports, matcher_matches, route_path_from_file};

    #[test]
    fn maps_route_groups_and_catch_all_segments() {
        assert_eq!(
            route_path_from_file("src/app/(api)/docs/[...slug]/route.ts"),
            "/docs/{slug*}"
        );
    }

    #[test]
    fn extracts_distinct_http_exports() {
        let source = "export async function GET() {}\nexport const PATCH = handler;";
        let methods = extract_http_exports(source)
            .into_iter()
            .map(|(method, _)| method)
            .collect::<Vec<_>>();
        assert_eq!(methods, ["GET", "PATCH"]);
    }

    #[test]
    fn applies_static_proxy_prefixes() {
        assert!(matcher_matches("/api/:path*", "/api/users"));
        assert!(!matcher_matches("/admin/:path*", "/api/users"));
    }
}
