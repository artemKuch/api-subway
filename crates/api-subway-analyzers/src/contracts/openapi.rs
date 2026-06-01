use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};

use api_subway_core::{
    ApiSchema, Confidence, ContentContract, Diagnostic, DiagnosticSeverity, EndpointContract,
    EvidenceKind, LiteralKind, ParameterContract, ParameterLocation, RequestContract,
    ResponseContract, SchemaConstraints, SchemaKind, SchemaLiteral, SchemaProperty, SourceRef,
    schema_id,
};
use serde_json::{Map, Value};

use super::{ContractAnalysis, source_evidence};

const MAX_ENUM_VALUES: usize = 64;
const MAX_LITERAL_LENGTH: usize = 128;
const MAX_PATTERN_LENGTH: usize = 256;
const MAX_PARAMETERS: usize = 256;
const MAX_RESPONSES: usize = 100;
const MAX_CONTENT_TYPES: usize = 32;
const MAX_SCHEMA_DEPTH: usize = 64;
const MAX_SCHEMA_PROPERTIES: usize = 1_000;
const MAX_SCHEMA_VARIANTS: usize = 100;
const MAX_SCHEMAS_PER_OPERATION: usize = 5_000;

pub(crate) fn analyze<'document>(
    document: &'document Value,
    relative: &str,
    source: SourceRef,
    raw_path: &str,
    method: &str,
    path_item: &'document Map<String, Value>,
    operation: &'document Map<String, Value>,
) -> ContractAnalysis {
    let mut collector = Collector {
        document,
        relative,
        source: source.clone(),
        schemas: Vec::new(),
        known: BTreeSet::new(),
        building: BTreeSet::new(),
        diagnostics: Vec::new(),
        reported_budgets: BTreeSet::new(),
    };
    let operation_origin = format!(
        "#/paths/{}/{}",
        escape_pointer(raw_path),
        method.to_ascii_lowercase()
    );
    let mut parameters = BTreeMap::<(ParameterLocation, String), ParameterContract>::new();
    collector.collect_parameters(
        path_item.get("parameters"),
        &format!("#/paths/{}/parameters", escape_pointer(raw_path)),
        &mut parameters,
    );
    collector.collect_parameters(
        operation.get("parameters"),
        &format!("{operation_origin}/parameters"),
        &mut parameters,
    );
    let bodies = collector.collect_request_bodies(
        operation.get("requestBody"),
        &format!("{operation_origin}/requestBody"),
    );
    let responses = collector.collect_responses(
        operation.get("responses"),
        &format!("{operation_origin}/responses"),
    );
    let request = RequestContract {
        parameters: parameters.into_values().collect(),
        bodies,
    };
    let contract = (!request.is_empty() || !responses.is_empty()).then(|| EndpointContract {
        confidence: Confidence::Exact,
        request,
        responses,
        evidence: vec![source_evidence(
            EvidenceKind::OpenApi,
            "Request and response contract declared by OpenAPI",
            Some(source),
        )],
    });
    ContractAnalysis {
        contract,
        schemas: collector.schemas,
        diagnostics: collector.diagnostics,
    }
}

struct Collector<'a> {
    document: &'a Value,
    relative: &'a str,
    source: SourceRef,
    schemas: Vec<ApiSchema>,
    known: BTreeSet<String>,
    building: BTreeSet<String>,
    diagnostics: Vec<Diagnostic>,
    reported_budgets: BTreeSet<String>,
}

impl<'document> Collector<'document> {
    fn collect_parameters(
        &mut self,
        value: Option<&'document Value>,
        origin: &str,
        output: &mut BTreeMap<(ParameterLocation, String), ParameterContract>,
    ) {
        let Some(parameters) = value.and_then(Value::as_array) else {
            return;
        };
        if parameters.len() > MAX_PARAMETERS {
            self.report_budget(
                "openapi-parameter-budget",
                "OpenAPI operation parameters were truncated at 256 entries",
            );
        }
        for (index, parameter) in parameters.iter().take(MAX_PARAMETERS).enumerate() {
            let parameter_origin = format!("{origin}/{index}");
            let (parameter, parameter_origin) = self.resolve(parameter, &parameter_origin);
            let Some(parameter) = parameter.as_object() else {
                continue;
            };
            let Some(name) = parameter.get("name").and_then(Value::as_str) else {
                continue;
            };
            let Some(location) = parameter
                .get("in")
                .and_then(Value::as_str)
                .and_then(parameter_location)
            else {
                continue;
            };
            let schema = parameter.get("schema").or_else(|| {
                parameter
                    .get("content")
                    .and_then(Value::as_object)
                    .and_then(|content| content.values().next())
                    .and_then(|media| media.get("schema"))
            });
            let Some(schema) = schema else {
                continue;
            };
            let schema_id = self.collect_schema(schema, &format!("{parameter_origin}/schema"));
            let required = location == ParameterLocation::Path
                || parameter
                    .get("required")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            output.insert(
                (location, name.to_owned()),
                ParameterContract {
                    name: name.to_owned(),
                    location,
                    required,
                    schema_id,
                },
            );
        }
    }

    fn collect_request_bodies(
        &mut self,
        value: Option<&'document Value>,
        origin: &str,
    ) -> Vec<ContentContract> {
        let Some(value) = value else {
            return Vec::new();
        };
        let (body, body_origin) = self.resolve(value, origin);
        let required = body
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.collect_content(
            body.get("content"),
            &format!("{body_origin}/content"),
            required,
        )
    }

    fn collect_responses(
        &mut self,
        value: Option<&'document Value>,
        origin: &str,
    ) -> Vec<ResponseContract> {
        let Some(responses) = value.and_then(Value::as_object) else {
            return Vec::new();
        };
        if responses.len() > MAX_RESPONSES {
            self.report_budget(
                "openapi-response-budget",
                "OpenAPI operation responses were truncated at 100 entries",
            );
        }
        responses
            .iter()
            .take(MAX_RESPONSES)
            .map(|(status, response)| {
                let response_origin = format!("{origin}/{}", escape_pointer(status));
                let (response, response_origin) = self.resolve(response, &response_origin);
                ResponseContract {
                    status: status.to_owned(),
                    contents: self.collect_content(
                        response.get("content"),
                        &format!("{response_origin}/content"),
                        false,
                    ),
                }
            })
            .collect()
    }

    fn collect_content(
        &mut self,
        value: Option<&'document Value>,
        origin: &str,
        required: bool,
    ) -> Vec<ContentContract> {
        let Some(content) = value.and_then(Value::as_object) else {
            return Vec::new();
        };
        if content.len() > MAX_CONTENT_TYPES {
            self.report_budget(
                "openapi-content-budget",
                "OpenAPI content types were truncated at 32 entries",
            );
        }
        content
            .iter()
            .take(MAX_CONTENT_TYPES)
            .filter_map(|(media_type, media)| {
                let schema = media.get("schema")?;
                let schema_id = self.collect_schema(
                    schema,
                    &format!("{origin}/{}/schema", escape_pointer(media_type)),
                );
                Some(ContentContract {
                    media_type: media_type.to_owned(),
                    schema_id,
                    required,
                })
            })
            .collect()
    }

    fn collect_schema(&mut self, value: &'document Value, fallback_origin: &str) -> String {
        self.collect_schema_at(value, fallback_origin, 0)
    }

    fn collect_schema_at(
        &mut self,
        value: &'document Value,
        fallback_origin: &str,
        depth: usize,
    ) -> String {
        if depth >= MAX_SCHEMA_DEPTH {
            return self.budget_schema(
                "openapi-schema-depth",
                "OpenAPI schema expansion stopped at depth 64",
            );
        }
        if self.schemas.len() >= MAX_SCHEMAS_PER_OPERATION {
            return self.budget_schema(
                "openapi-schema-count-budget",
                "OpenAPI operation schema expansion stopped at 5000 schemas",
            );
        }
        let (value, origin) = self.resolve(value, fallback_origin);
        let id = schema_id(&format!("openapi:{}:{origin}", self.relative));
        if self.known.contains(&id) || self.building.contains(&id) {
            return id;
        }
        self.building.insert(id.clone());
        let required = value
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<BTreeSet<_>>();
        let properties = value.get("properties").and_then(Value::as_object);
        if properties.is_some_and(|properties| properties.len() > MAX_SCHEMA_PROPERTIES) {
            self.report_budget(
                "openapi-schema-property-budget",
                "OpenAPI schema properties were truncated at 1000 entries",
            );
        }
        let properties = properties
            .map(|properties| {
                properties
                    .iter()
                    .take(MAX_SCHEMA_PROPERTIES)
                    .map(|(name, property)| SchemaProperty {
                        name: name.clone(),
                        schema_id: self.collect_schema_at(
                            property,
                            &format!("{origin}/properties/{}", escape_pointer(name)),
                            depth + 1,
                        ),
                        required: required.contains(name.as_str()),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let items = value
            .get("items")
            .map(|items| self.collect_schema_at(items, &format!("{origin}/items"), depth + 1));
        let (kind, variants) = if let Some(variants) = value.get("allOf").and_then(Value::as_array)
        {
            (
                SchemaKind::Intersection,
                self.collect_variants(variants, &format!("{origin}/allOf"), depth + 1),
            )
        } else if let Some(variants) = value
            .get("oneOf")
            .or_else(|| value.get("anyOf"))
            .and_then(Value::as_array)
        {
            (
                SchemaKind::Union,
                self.collect_variants(variants, &format!("{origin}/variants"), depth + 1),
            )
        } else {
            (
                schema_kind(value, !properties.is_empty(), items.is_some()),
                Vec::new(),
            )
        };
        let nullable = value
            .get("nullable")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || value
                .get("type")
                .and_then(Value::as_array)
                .is_some_and(|types| types.iter().any(|value| value.as_str() == Some("null")));
        let enum_values = value
            .get("enum")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(MAX_ENUM_VALUES)
            .filter_map(schema_literal)
            .collect();
        let schema = ApiSchema {
            id: id.clone(),
            name: schema_name(&origin),
            kind,
            format: value
                .get("format")
                .and_then(Value::as_str)
                .map(str::to_owned),
            properties,
            items,
            variants,
            enum_values,
            const_value: value.get("const").and_then(schema_literal),
            constraints: constraints(value),
            nullable,
            confidence: Confidence::Exact,
            evidence: vec![source_evidence(
                EvidenceKind::OpenApi,
                "Schema declared by OpenAPI",
                Some(self.source.clone()),
            )],
        };
        self.schemas.push(schema);
        self.building.remove(&id);
        self.known.insert(id.clone());
        id
    }

    fn collect_variants(
        &mut self,
        values: &'document [Value],
        origin: &str,
        depth: usize,
    ) -> Vec<String> {
        if values.len() > MAX_SCHEMA_VARIANTS {
            self.report_budget(
                "openapi-schema-variant-budget",
                "OpenAPI schema variants were truncated at 100 entries",
            );
        }
        values
            .iter()
            .take(MAX_SCHEMA_VARIANTS)
            .enumerate()
            .map(|(index, value)| {
                self.collect_schema_at(value, &format!("{origin}/{index}"), depth)
            })
            .collect()
    }

    fn budget_schema(&mut self, code: &str, message: &str) -> String {
        self.report_budget(code, message);
        let id = schema_id(&format!(
            "openapi:{}:#/api-subway-budget/{code}",
            self.relative
        ));
        if self.known.insert(id.clone()) {
            self.schemas.push(ApiSchema {
                id: id.clone(),
                name: None,
                kind: SchemaKind::Unknown,
                format: None,
                properties: Vec::new(),
                items: None,
                variants: Vec::new(),
                enum_values: Vec::new(),
                const_value: None,
                constraints: SchemaConstraints::default(),
                nullable: false,
                confidence: Confidence::Inferred,
                evidence: vec![source_evidence(
                    EvidenceKind::OpenApi,
                    message,
                    Some(self.source.clone()),
                )],
            });
        }
        id
    }

    fn report_budget(&mut self, code: &str, message: &str) {
        if self.reported_budgets.insert(code.to_owned()) {
            self.diagnostics.push(Diagnostic {
                code: code.to_owned(),
                severity: DiagnosticSeverity::Warning,
                message: message.to_owned(),
                source: Some(self.source.clone()),
            });
        }
    }

    fn resolve(
        &mut self,
        value: &'document Value,
        fallback_origin: &str,
    ) -> (&'document Value, String) {
        static EMPTY: LazyLock<Value> = LazyLock::new(|| Value::Object(Map::new()));

        let mut current = value;
        let mut origin = fallback_origin.to_owned();
        let mut seen = BTreeSet::new();
        for _ in 0..32 {
            let Some(reference) = current
                .get("$ref")
                .and_then(Value::as_str)
                .map(str::to_owned)
            else {
                return (current, origin);
            };
            if !reference.starts_with("#/") {
                self.add_ref_diagnostic(
                    "openapi-external-ref",
                    "External OpenAPI $ref was not resolved",
                );
                return (&EMPTY, origin);
            }
            if !seen.insert(reference.clone()) {
                self.add_ref_diagnostic(
                    "openapi-ref-cycle",
                    "OpenAPI $ref alias cycle was not expanded",
                );
                return (&EMPTY, origin);
            }
            let Some(target) = self.document.pointer(&reference[1..]) else {
                self.add_ref_diagnostic(
                    "openapi-unresolved-ref",
                    "Local OpenAPI $ref could not be resolved",
                );
                return (&EMPTY, origin);
            };
            current = target;
            origin = reference;
        }
        self.add_ref_diagnostic(
            "openapi-ref-depth",
            "OpenAPI $ref chain exceeded the depth budget",
        );
        (&EMPTY, origin)
    }

    fn add_ref_diagnostic(&mut self, code: &str, message: &str) {
        self.diagnostics.push(Diagnostic {
            code: code.to_owned(),
            severity: DiagnosticSeverity::Warning,
            message: message.to_owned(),
            source: Some(self.source.clone()),
        });
    }
}

fn parameter_location(value: &str) -> Option<ParameterLocation> {
    match value {
        "path" => Some(ParameterLocation::Path),
        "query" => Some(ParameterLocation::Query),
        "header" => Some(ParameterLocation::Header),
        "cookie" => Some(ParameterLocation::Cookie),
        _ => None,
    }
}

fn schema_kind(value: &Value, has_properties: bool, has_items: bool) -> SchemaKind {
    let declared = value.get("type").and_then(|value| {
        value.as_str().or_else(|| {
            value
                .as_array()?
                .iter()
                .filter_map(Value::as_str)
                .find(|value| *value != "null")
        })
    });
    match declared {
        Some("object") => SchemaKind::Object,
        Some("array") => SchemaKind::Array,
        Some("string") => SchemaKind::String,
        Some("integer") => SchemaKind::Integer,
        Some("number") => SchemaKind::Number,
        Some("boolean") => SchemaKind::Boolean,
        Some("null") => SchemaKind::Null,
        _ if has_properties => SchemaKind::Object,
        _ if has_items => SchemaKind::Array,
        _ => SchemaKind::Unknown,
    }
}

fn constraints(value: &Value) -> SchemaConstraints {
    SchemaConstraints {
        min_length: value.get("minLength").and_then(Value::as_u64),
        max_length: value.get("maxLength").and_then(Value::as_u64),
        minimum: value.get("minimum").and_then(number_string),
        maximum: value.get("maximum").and_then(number_string),
        min_items: value.get("minItems").and_then(Value::as_u64),
        max_items: value.get("maxItems").and_then(Value::as_u64),
        pattern: value
            .get("pattern")
            .and_then(Value::as_str)
            .filter(|pattern| pattern.len() <= MAX_PATTERN_LENGTH)
            .map(str::to_owned),
    }
}

fn number_string(value: &Value) -> Option<String> {
    value.as_number().map(ToString::to_string)
}

fn schema_literal(value: &Value) -> Option<SchemaLiteral> {
    let (kind, value) = match value {
        Value::String(value) if value.len() <= MAX_LITERAL_LENGTH => {
            (LiteralKind::String, value.clone())
        }
        Value::Number(value) if value.is_i64() || value.is_u64() => {
            (LiteralKind::Integer, value.to_string())
        }
        Value::Number(value) => (LiteralKind::Number, value.to_string()),
        Value::Bool(value) => (LiteralKind::Boolean, value.to_string()),
        Value::Null => (LiteralKind::Null, "null".to_owned()),
        _ => return None,
    };
    Some(SchemaLiteral { kind, value })
}

fn schema_name(origin: &str) -> Option<String> {
    origin
        .strip_prefix("#/components/schemas/")
        .filter(|name| !name.contains('/'))
        .map(unescape_pointer)
}

fn escape_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn unescape_pointer(value: &str) -> String {
    value.replace("~1", "/").replace("~0", "~")
}

#[cfg(test)]
mod tests {
    use api_subway_core::SourceRef;
    use serde_json::json;

    use super::analyze;

    #[test]
    fn resolves_request_response_and_recursive_refs() {
        let document = json!({
            "openapi": "3.1.0",
            "paths": {
                "/users/{id}": {
                    "get": {
                        "parameters": [{
                            "name": "id", "in": "path", "required": true,
                            "schema": { "type": "string", "format": "uuid" }
                        }],
                        "responses": {
                            "200": { "content": { "application/json": {
                                "schema": { "$ref": "#/components/schemas/User" }
                            } } }
                        }
                    }
                }
            },
            "components": { "schemas": { "User": {
                "type": "object", "required": ["id"],
                "properties": {
                    "id": { "type": "string" },
                    "manager": { "$ref": "#/components/schemas/User" }
                }
            } } }
        });
        let path_item = document["paths"]["/users/{id}"]
            .as_object()
            .expect("path item");
        let operation = path_item["get"].as_object().expect("operation");
        let result = analyze(
            &document,
            "openapi.yaml",
            SourceRef {
                file: "openapi.yaml".to_owned(),
                line: 1,
                column: 1,
            },
            "/users/{id}",
            "get",
            path_item,
            operation,
        );
        let contract = result.contract.expect("contract");
        assert_eq!(contract.request.parameters.len(), 1);
        assert_eq!(contract.responses[0].status, "200");
        assert!(
            result
                .schemas
                .iter()
                .any(|schema| schema.name.as_deref() == Some("User"))
        );
    }

    #[test]
    fn bounds_inline_schema_graph_expansion() {
        let properties = (0..1_005)
            .map(|index| (format!("field_{index}"), json!({ "type": "string" })))
            .collect::<serde_json::Map<_, _>>();
        let mut deep_schema = json!({ "type": "string" });
        for _ in 0..70 {
            deep_schema = json!({
                "type": "object",
                "required": ["child"],
                "properties": { "child": deep_schema }
            });
        }
        let document = json!({
            "openapi": "3.1.0",
            "paths": { "/bounded": { "get": { "responses": {
                "200": { "content": { "application/json": { "schema": {
                    "type": "object",
                    "properties": properties
                } } } },
                "201": { "content": { "application/json": { "schema": deep_schema } } }
            } } } }
        });
        let path_item = document["paths"]["/bounded"]
            .as_object()
            .expect("path item");
        let operation = path_item["get"].as_object().expect("operation");
        let result = analyze(
            &document,
            "openapi.json",
            SourceRef {
                file: "openapi.json".to_owned(),
                line: 1,
                column: 1,
            },
            "/bounded",
            "get",
            path_item,
            operation,
        );

        assert!(
            result
                .schemas
                .iter()
                .any(|schema| schema.properties.len() == 1_000)
        );
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.code == "openapi-schema-property-budget" })
        );
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "openapi-schema-depth")
        );
    }
}
