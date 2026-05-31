use std::{collections::BTreeMap, fs, path::Path};

use api_subway_core::{
    ApiMapBuilder, Confidence, Dependency, DependencyKind, Endpoint, Evidence, EvidenceKind,
    Relation, SourceRef, canonical_endpoint_id_for_normalized_path, dependency_id,
    district_for_normalized_path, normalize_openapi_route_path,
};
use serde::de::{self, DeserializeSeed, Deserializer, Error as _, MapAccess, SeqAccess, Visitor};
use serde_json::Value;

use crate::{
    contracts,
    discovery::{AnalyzerError, relative_source_path},
    input::{ReadTextError, read_text_bounded},
};

const MAX_OPENAPI_BYTES: u64 = 16 * 1024 * 1024;
const MAX_OPENAPI_SCALAR_BYTES: usize = 16 * 1024 * 1024;
const MAX_YAML_EVENTS: usize = 500_000;
const MAX_YAML_NODES: usize = 200_000;
const MAX_YAML_ALIASES: usize = 2_048;
const MAX_YAML_ANCHORS: usize = 2_048;
const MAX_YAML_DEPTH: usize = 64;
const MAX_YAML_ALIAS_REPLAY_EVENTS: usize = 200_000;
const MAX_YAML_ALIAS_REPLAY_DEPTH: usize = 32;
const MAX_YAML_ALIAS_EXPANSIONS: usize = 128;
const METHODS: &[&str] = &[
    "get", "post", "put", "patch", "delete", "options", "head", "trace",
];

pub(crate) fn merge(
    root: &Path,
    path: &Path,
    builder: &mut ApiMapBuilder,
) -> Result<(), AnalyzerError> {
    let canonical = path
        .canonicalize()
        .map_err(|source| AnalyzerError::OpenApiRead {
            path: path.to_path_buf(),
            source,
        })?;
    if !canonical.starts_with(root) {
        return Err(AnalyzerError::OpenApiOutsideRoot(path.to_path_buf()));
    }
    let metadata =
        fs::symlink_metadata(&canonical).map_err(|source| AnalyzerError::OpenApiRead {
            path: path.to_path_buf(),
            source,
        })?;
    if !metadata.file_type().is_file() {
        return Err(AnalyzerError::OpenApiNotFile(path.to_path_buf()));
    }
    let contents =
        read_text_bounded(&canonical, MAX_OPENAPI_BYTES).map_err(|error| match error {
            ReadTextError::Budget => AnalyzerError::OpenApiBudget(path.to_path_buf()),
            ReadTextError::Io(source) => AnalyzerError::OpenApiRead {
                path: path.to_path_buf(),
                source,
            },
        })?;
    let is_json = canonical
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"));
    let document =
        parse_document(&contents, is_json).map_err(|message| AnalyzerError::OpenApiParse {
            path: path.to_path_buf(),
            message,
        })?;
    let version = document
        .get("openapi")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !(version.starts_with("3.0.") || version.starts_with("3.1.")) {
        return Err(AnalyzerError::OpenApiParse {
            path: path.to_path_buf(),
            message: format!("expected OpenAPI 3.0 or 3.1, found '{version}'"),
        });
    }
    let relative = relative_source_path(root, &canonical).unwrap_or_else(|| "openapi".to_owned());
    let operation_sources = index_operation_sources(&contents, &relative);
    let global_security = document.get("security");
    let Some(paths) = document.get("paths").and_then(Value::as_object) else {
        return Ok(());
    };
    for (raw_path, path_item) in paths {
        let Some(path_item) = path_item.as_object() else {
            continue;
        };
        for method in METHODS {
            let Some(operation) = path_item.get(*method).and_then(Value::as_object) else {
                continue;
            };
            let path = normalize_openapi_route_path(raw_path);
            let method = method.to_ascii_uppercase();
            let endpoint_id = canonical_endpoint_id_for_normalized_path(&method, &path);
            let source = operation_sources
                .get(&(raw_path.clone(), method.to_ascii_lowercase()))
                .cloned()
                .unwrap_or_else(|| SourceRef {
                    file: relative.clone(),
                    line: 1,
                    column: 1,
                });
            let contract = contracts::openapi::analyze(
                &document,
                &relative,
                source.clone(),
                raw_path,
                &method,
                path_item,
                operation,
            );
            for schema in contract.schemas {
                builder.add_schema(schema);
            }
            for diagnostic in contract.diagnostics {
                builder.add_diagnostic(diagnostic);
            }
            let tags = operation
                .get("tags")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>();
            builder.add_endpoint(Endpoint {
                id: endpoint_id.clone(),
                method,
                path: path.clone(),
                display_path: path.clone(),
                district: district_for_normalized_path(&path),
                framework: "openapi".to_owned(),
                operation_id: operation
                    .get("operationId")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                tags,
                sources: vec![source.clone()],
                spec_only: true,
                contract: contract.contract,
            });
            let security = operation.get("security").or(global_security);
            for scheme in security_scheme_names(security) {
                let dependency_id = dependency_id(DependencyKind::Middleware, &scheme);
                builder.add_dependency(Dependency {
                    id: dependency_id.clone(),
                    name: scheme.clone(),
                    kind: DependencyKind::Middleware,
                    pinned: false,
                    packages: Vec::new(),
                });
                builder.add_relation(Relation {
                    endpoint_id: endpoint_id.clone(),
                    dependency_id,
                    confidence: Confidence::Exact,
                    evidence: vec![Evidence {
                        kind: EvidenceKind::OpenApi,
                        detail: format!("OpenAPI security requirement: {scheme}"),
                        source: Some(source.clone()),
                    }],
                });
            }
        }
    }
    Ok(())
}

fn parse_document(contents: &str, is_json: bool) -> Result<Value, String> {
    if is_json {
        let mut deserializer = serde_json::Deserializer::from_str(contents);
        let mut budget = JsonBudget::default();
        let value = UniqueJsonSeed {
            budget: &mut budget,
            depth: 0,
        }
        .deserialize(&mut deserializer)
        .map_err(|error| error.to_string())?;
        deserializer.end().map_err(|error| error.to_string())?;
        return Ok(value);
    }
    let options = serde_saphyr::options! {
        budget: serde_saphyr::budget! {
            max_events: MAX_YAML_EVENTS,
            max_aliases: MAX_YAML_ALIASES,
            max_anchors: MAX_YAML_ANCHORS,
            max_depth: MAX_YAML_DEPTH,
            max_inclusion_depth: 0,
            max_documents: 1,
            max_nodes: MAX_YAML_NODES,
            max_total_scalar_bytes: MAX_OPENAPI_SCALAR_BYTES,
            max_total_comment_bytes: MAX_OPENAPI_SCALAR_BYTES / 4,
            max_merge_keys: 1_024,
        },
        duplicate_keys: serde_saphyr::DuplicateKeyPolicy::Error,
        alias_limits: serde_saphyr::alias_limits! {
            max_total_replayed_events: MAX_YAML_ALIAS_REPLAY_EVENTS,
            max_replay_stack_depth: MAX_YAML_ALIAS_REPLAY_DEPTH,
            max_alias_expansions_per_anchor: MAX_YAML_ALIAS_EXPANSIONS,
        },
        strict_booleans: true,
    };
    serde_saphyr::from_str_with_options(contents, options).map_err(|error| error.to_string())
}

#[derive(Default)]
struct JsonBudget {
    nodes: usize,
    scalar_bytes: usize,
}

impl JsonBudget {
    fn consume_node<E: de::Error>(&mut self) -> Result<(), E> {
        if self.nodes >= MAX_YAML_NODES {
            return Err(E::custom(format!(
                "JSON document exceeds the {MAX_YAML_NODES}-node budget"
            )));
        }
        self.nodes += 1;
        Ok(())
    }

    fn consume_scalar<E: de::Error>(&mut self, bytes: usize) -> Result<(), E> {
        self.scalar_bytes = self.scalar_bytes.checked_add(bytes).ok_or_else(|| {
            E::custom("JSON scalar byte accounting overflowed the supported budget")
        })?;
        if self.scalar_bytes > MAX_OPENAPI_SCALAR_BYTES {
            return Err(E::custom(format!(
                "JSON document exceeds the {MAX_OPENAPI_SCALAR_BYTES}-byte scalar budget"
            )));
        }
        Ok(())
    }
}

struct UniqueJsonSeed<'a> {
    budget: &'a mut JsonBudget,
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for UniqueJsonSeed<'_> {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        if self.depth > MAX_YAML_DEPTH {
            return Err(D::Error::custom(format!(
                "JSON document exceeds the {MAX_YAML_DEPTH}-level depth budget"
            )));
        }
        self.budget.consume_node::<D::Error>()?;
        deserializer.deserialize_any(UniqueJsonVisitor {
            budget: self.budget,
            depth: self.depth,
        })
    }
}

struct UniqueJsonVisitor<'a> {
    budget: &'a mut JsonBudget,
    depth: usize,
}

impl<'de> Visitor<'de> for UniqueJsonVisitor<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("JSON numbers must be finite"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.budget.consume_scalar::<E>(value.len())?;
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.budget.consume_scalar::<E>(value.len())?;
        Ok(Value::String(value))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(UniqueJsonSeed {
            budget: &mut *self.budget,
            depth: self.depth + 1,
        })? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = serde_json::Map::new();
        while let Some(key) = object.next_key::<String>()? {
            self.budget.consume_node::<A::Error>()?;
            self.budget.consume_scalar::<A::Error>(key.len())?;
            if values.contains_key(&key) {
                return Err(A::Error::custom(format!("duplicate object key '{key}'")));
            }
            let value = object.next_value_seed(UniqueJsonSeed {
                budget: &mut *self.budget,
                depth: self.depth + 1,
            })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

#[cfg(test)]
fn operation_source(contents: &str, relative: &str, path: &str, method: &str) -> SourceRef {
    index_operation_sources(contents, relative)
        .remove(&(path.to_owned(), method.to_ascii_lowercase()))
        .unwrap_or_else(|| SourceRef {
            file: relative.to_owned(),
            line: 1,
            column: 1,
        })
}

fn index_operation_sources(
    contents: &str,
    relative: &str,
) -> BTreeMap<(String, String), SourceRef> {
    let mut output = BTreeMap::new();
    let mut paths_indent = None;
    let mut current_path: Option<(String, usize)> = None;
    for (index, line) in contents.lines().enumerate() {
        let indentation = line.len().saturating_sub(line.trim_start().len());
        let Some(key) = mapping_key(line) else {
            continue;
        };
        if paths_indent.is_none() && key == "paths" {
            paths_indent = Some(indentation);
            continue;
        }
        let Some(root_indent) = paths_indent else {
            continue;
        };
        if indentation <= root_indent && key != "paths" {
            paths_indent = None;
            current_path = None;
            continue;
        }
        if key.starts_with('/') {
            current_path = Some((key, indentation));
            continue;
        }
        if let Some((path, path_indent)) = &current_path {
            if indentation <= *path_indent {
                current_path = None;
            } else {
                let method = key.to_ascii_lowercase();
                if !METHODS.contains(&method.as_str()) {
                    continue;
                }
                let column = line
                    .find(|character: char| !character.is_whitespace())
                    .map_or(1, |offset| offset.saturating_add(1));
                output.insert(
                    (path.clone(), method),
                    SourceRef {
                        file: relative.to_owned(),
                        line: u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX),
                        column: u32::try_from(column).unwrap_or(u32::MAX),
                    },
                );
            }
        }
    }
    output
}

fn mapping_key(line: &str) -> Option<String> {
    let mut text = line.trim_start().trim_start_matches(',').trim_start();
    if text.is_empty() || text.starts_with('#') || matches!(text, "{" | "}") {
        return None;
    }
    if let Some(rest) = text.strip_prefix('"') {
        let mut escaped = false;
        for (index, character) in rest.char_indices() {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                let key = serde_json::from_str::<String>(&text[..index.saturating_add(2)]).ok()?;
                let after = rest[index.saturating_add(1)..].trim_start();
                return after.starts_with(':').then_some(key);
            }
        }
        return None;
    }
    if let Some(rest) = text.strip_prefix('\'') {
        let end = rest.find('\'')?;
        let after = rest[end.saturating_add(1)..].trim_start();
        return after
            .starts_with(':')
            .then(|| rest[..end].replace("\\'", "'"));
    }
    let colon = text.find(':')?;
    text = text[..colon].trim_end();
    (!text.is_empty()).then(|| text.to_owned())
}

fn security_scheme_names(value: Option<&Value>) -> Vec<String> {
    let mut output = value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .flat_map(|requirement| requirement.keys().cloned())
        .collect::<Vec<_>>();
    output.sort();
    output.dedup();
    output
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use api_subway_core::ApiMapBuilder;
    use serde_json::json;

    use super::{
        MAX_YAML_DEPTH, MAX_YAML_NODES, merge, operation_source, parse_document,
        security_scheme_names,
    };

    #[test]
    fn reads_distinct_security_requirements() {
        let security = json!([{"bearerAuth": []}, {"apiKey": []}, {"bearerAuth": []}]);
        assert_eq!(
            security_scheme_names(Some(&security)),
            ["apiKey", "bearerAuth"]
        );
    }

    #[test]
    fn locates_yaml_and_json_operation_lines() {
        let yaml = "openapi: 3.1.0\npaths:\n  /users:\n    get:\n      responses: {}\n";
        assert_eq!(operation_source(yaml, "api.yaml", "/users", "GET").line, 4);

        let json = "{\n  \"paths\": {\n    \"/users\": {\n      \"post\": {}\n    }\n  }\n}";
        assert_eq!(operation_source(json, "api.json", "/users", "POST").line, 4);
    }

    #[test]
    fn enforces_yaml_structure_and_alias_budgets() {
        let duplicate = "openapi: 3.1.0\nopenapi: 3.0.3\npaths: {}\n";
        assert!(parse_document(duplicate, false).is_err());

        let mut deep = String::from("root:\n");
        for depth in 0..70 {
            writeln!(deep, "{}level_{depth}:", "  ".repeat(depth + 1))
                .expect("writing to a String should succeed");
        }
        writeln!(deep, "{}value: true", "  ".repeat(71))
            .expect("writing to a String should succeed");
        assert!(parse_document(&deep, false).is_err());

        let mut aliases = String::from("openapi: 3.1.0\nbase: &base { type: string }\nitems:\n");
        for _ in 0..=128 {
            aliases.push_str("  - *base\n");
        }
        assert!(parse_document(&aliases, false).is_err());
    }

    #[test]
    fn rejects_duplicate_json_object_keys_at_any_depth() {
        assert!(parse_document(r#"{"openapi":"3.1.0","openapi":"3.0.3"}"#, true).is_err());
        assert!(
            parse_document(
                r#"{"openapi":"3.1.0","paths":{"/users":{"get":{},"get":{}}}}"#,
                true,
            )
            .is_err()
        );
    }

    #[test]
    fn enforces_json_structure_budgets_while_deserializing() {
        let too_deep = format!(
            "{}null{}",
            "[".repeat(MAX_YAML_DEPTH + 1),
            "]".repeat(MAX_YAML_DEPTH + 1)
        );
        assert!(parse_document(&too_deep, true).is_err());

        let mut too_wide = String::with_capacity(MAX_YAML_NODES * 5);
        too_wide.push('[');
        for index in 0..MAX_YAML_NODES {
            if index > 0 {
                too_wide.push(',');
            }
            too_wide.push_str("null");
        }
        too_wide.push(']');
        assert!(parse_document(&too_wide, true).is_err());
    }

    #[test]
    fn accepts_bounded_yaml_anchors() {
        let document = parse_document(
            "openapi: 3.1.0\nbase: &base {type: string}\nschema: *base\npaths: {}\n",
            false,
        )
        .expect("bounded anchors should remain supported");
        assert_eq!(document["schema"]["type"], "string");
    }

    #[test]
    fn rejects_non_file_openapi_inputs_before_reading() {
        let root = tempfile::tempdir().expect("temporary OpenAPI root");
        let directory = root.path().join("api.json");
        std::fs::create_dir(&directory).expect("directory-shaped OpenAPI input");
        let canonical_root = root.path().canonicalize().expect("canonical OpenAPI root");
        let mut builder = ApiMapBuilder::new("test");
        assert!(matches!(
            merge(&canonical_root, &directory, &mut builder),
            Err(crate::discovery::AnalyzerError::OpenApiNotFile(path)) if path == directory
        ));
    }
}
