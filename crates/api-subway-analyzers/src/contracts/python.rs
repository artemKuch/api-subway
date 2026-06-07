use std::{collections::BTreeSet, sync::LazyLock};

use api_subway_core::{
    ApiSchema, Confidence, ContentContract, Diagnostic, DiagnosticSeverity, EndpointContract,
    EvidenceKind, LiteralKind, ParameterContract, ParameterLocation, RequestContract,
    ResponseContract, SchemaConstraints, SchemaKind, SchemaLiteral, SchemaProperty, SourceRef,
    schema_id,
};
use regex::Regex;

use crate::{
    discovery::RouteRecord,
    python::{PythonFile, PythonIndex},
};

use super::{ContractAnalysis, source_evidence};

const MAX_SCHEMAS_PER_ROUTE: usize = 1_000;
const MAX_SCHEMA_RECURSION: usize = 64;
const MAX_SCHEMA_MEMBERS: usize = 1_000;
const MAX_SCHEMA_WORK: usize = 10_000;

pub(crate) fn analyze_route(route: &RouteRecord, index: &PythonIndex) -> ContractAnalysis {
    let Some(file) = index.get(&route.source_path) else {
        return ContractAnalysis::default();
    };
    let Some(symbol) = route.entry_symbols.first() else {
        return ContractAnalysis::default();
    };
    let Some(signature) = function_signature(&file.source, symbol) else {
        return ContractAnalysis::default();
    };
    let source = route.endpoint.sources.first().cloned();
    let mut collector = Collector {
        index,
        schemas: Vec::new(),
        known: BTreeSet::new(),
        building: BTreeSet::new(),
        diagnostics: Vec::new(),
        work: 0,
        budget_reported: false,
    };
    let mut parameters = Vec::new();
    let mut bodies = Vec::new();
    let mut responses = Vec::new();
    let mut evidence = Vec::new();
    let mut confidence = Confidence::Exact;
    for parameter in split_top_level(&signature.parameters) {
        let Some(parameter) = parse_parameter(parameter) else {
            continue;
        };
        if parameter.name == "self"
            || parameter.annotation.contains("Request")
            || parameter
                .default
                .as_deref()
                .is_some_and(|value| value.contains("Depends("))
        {
            continue;
        }
        let annotation = annotated_base(&parameter.annotation);
        let origin = format!(
            "python:{}:{symbol}:parameter:{}",
            file.relative, parameter.name
        );
        let Some(schema_id) = collector.collect_type(
            file,
            annotation,
            &origin,
            Some(parameter.name.clone()),
            source.clone(),
        ) else {
            continue;
        };
        let location = parameter_location(route, &parameter, annotation, index, file);
        let required = parameter.required() || location == Some(ParameterLocation::Path);
        match location {
            Some(location) => parameters.push(ParameterContract {
                name: parameter.name,
                location,
                required,
                schema_id,
            }),
            None => bodies.push(ContentContract {
                media_type: "application/json".to_owned(),
                schema_id,
                required,
            }),
        }
        evidence.push(source_evidence(
            EvidenceKind::Framework,
            "FastAPI request contract resolved from a handler annotation",
            source.clone(),
        ));
    }
    let decorator = decorator_before(&file.source, signature.start);
    let response_annotation = decorator
        .as_deref()
        .and_then(|value| named_decorator_argument(value, "response_model"))
        .or(signature.return_type.as_deref())
        .filter(|annotation| !is_ignored_response_type(annotation));
    if let Some(annotation) = response_annotation {
        let origin = format!("python:{}:{symbol}:response", file.relative);
        if let Some(schema_id) = collector.collect_type(
            file,
            annotated_base(annotation),
            &origin,
            Some(format!("{symbol} response")),
            source.clone(),
        ) {
            responses.push(ResponseContract {
                status: decorator
                    .as_deref()
                    .and_then(|value| named_decorator_argument(value, "status_code"))
                    .and_then(status_code)
                    .unwrap_or_else(|| "200".to_owned()),
                contents: vec![ContentContract {
                    media_type: "application/json".to_owned(),
                    schema_id,
                    required: false,
                }],
            });
            evidence.push(source_evidence(
                EvidenceKind::Framework,
                "FastAPI response contract resolved from response_model or return annotation",
                source.clone(),
            ));
        }
    }
    if collector
        .schemas
        .iter()
        .any(|schema| schema.confidence == Confidence::Inferred)
    {
        confidence = Confidence::Inferred;
    }
    let request = RequestContract { parameters, bodies };
    let contract = (!request.is_empty() || !responses.is_empty()).then_some(EndpointContract {
        confidence,
        request,
        responses,
        evidence,
    });
    ContractAnalysis {
        contract,
        schemas: collector.schemas,
        diagnostics: collector.diagnostics,
    }
}

struct FunctionSignature {
    start: usize,
    parameters: String,
    return_type: Option<String>,
}

fn function_signature(source: &str, symbol: &str) -> Option<FunctionSignature> {
    let declaration = Regex::new(&format!(
        r"(?m)^[ \t]*(?:async\s+)?def\s+{}\s*\(",
        regex::escape(symbol)
    ))
    .ok()?
    .find(source)?;
    let open = declaration.end() - 1;
    let (parameters, close) = call_body(source, open)?;
    let tail = source.get(close + 1..)?;
    let header_end = tail.find(':')?;
    let return_type = tail[..header_end]
        .trim()
        .strip_prefix("->")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    Some(FunctionSignature {
        start: declaration.start(),
        parameters: parameters.to_owned(),
        return_type,
    })
}

struct FunctionParameter {
    name: String,
    annotation: String,
    default: Option<String>,
}

impl FunctionParameter {
    fn required(&self) -> bool {
        self.default
            .as_deref()
            .is_none_or(|value| value.trim() == "...")
            && !self.annotation.contains("Optional[")
            && !self.annotation.contains("| None")
    }
}

fn parse_parameter(value: &str) -> Option<FunctionParameter> {
    let value = value.trim().trim_start_matches('*');
    let colon = top_level_delimiter(value, b':')?;
    let name = value[..colon].trim();
    if name.is_empty() {
        return None;
    }
    let remainder = value[colon + 1..].trim();
    let (annotation, default) = top_level_delimiter(remainder, b'=').map_or_else(
        || (remainder, None),
        |equals| {
            (
                remainder[..equals].trim(),
                Some(remainder[equals + 1..].trim()),
            )
        },
    );
    Some(FunctionParameter {
        name: name.to_owned(),
        annotation: annotation.to_owned(),
        default: default.map(str::to_owned),
    })
}

fn parameter_location(
    route: &RouteRecord,
    parameter: &FunctionParameter,
    annotation: &str,
    index: &PythonIndex,
    file: &PythonFile,
) -> Option<ParameterLocation> {
    if route
        .endpoint
        .path
        .contains(&format!("{{{}}}", parameter.name))
    {
        return Some(ParameterLocation::Path);
    }
    let marker = format!(
        "{} {}",
        parameter.annotation,
        parameter.default.as_deref().unwrap_or("")
    );
    if marker.contains("Path(") {
        return Some(ParameterLocation::Path);
    }
    if marker.contains("Query(") {
        return Some(ParameterLocation::Query);
    }
    if marker.contains("Header(") {
        return Some(ParameterLocation::Header);
    }
    if marker.contains("Cookie(") {
        return Some(ParameterLocation::Cookie);
    }
    (!is_model_type(file, annotation, index)).then_some(ParameterLocation::Query)
}

struct Collector<'a> {
    index: &'a PythonIndex,
    schemas: Vec<ApiSchema>,
    known: BTreeSet<String>,
    building: BTreeSet<String>,
    diagnostics: Vec<Diagnostic>,
    work: usize,
    budget_reported: bool,
}

impl Collector<'_> {
    fn collect_type(
        &mut self,
        file: &PythonFile,
        annotation: &str,
        origin: &str,
        name: Option<String>,
        source: Option<SourceRef>,
    ) -> Option<String> {
        self.work = self.work.saturating_add(1);
        if self.work > MAX_SCHEMA_WORK
            || self.schemas.len() >= MAX_SCHEMAS_PER_ROUTE
            || self.building.len() >= MAX_SCHEMA_RECURSION
        {
            self.report_budget(source);
            return None;
        }
        let annotation = annotation.trim().trim_matches(['\'', '"']);
        let id = schema_id(origin);
        if self.known.contains(&id) || self.building.contains(&id) {
            return Some(id);
        }
        self.building.insert(id.clone());
        let (annotation, nullable) = optional_inner(annotation);
        if let Some(inner) = generic_inner(annotation, &["list", "List", "Sequence", "set", "Set"])
        {
            let item = self.collect_type(
                file,
                inner,
                &format!("{origin}:items"),
                None,
                source.clone(),
            );
            self.push_schema(ApiSchema {
                id: id.clone(),
                name,
                kind: SchemaKind::Array,
                format: None,
                properties: Vec::new(),
                items: item,
                variants: Vec::new(),
                enum_values: Vec::new(),
                const_value: None,
                constraints: SchemaConstraints::default(),
                nullable,
                confidence: Confidence::Exact,
                evidence: schema_evidence(source),
            });
            return Some(id);
        }
        if let Some(inner) = generic_inner(annotation, &["Union"]) {
            let variants = split_top_level(inner)
                .into_iter()
                .take(MAX_SCHEMA_MEMBERS)
                .filter(|value| value.trim() != "None")
                .enumerate()
                .filter_map(|(index, value)| {
                    self.collect_type(
                        file,
                        value,
                        &format!("{origin}:variant:{index}"),
                        None,
                        source.clone(),
                    )
                })
                .collect();
            self.push_schema(ApiSchema {
                id: id.clone(),
                name,
                kind: SchemaKind::Union,
                format: None,
                properties: Vec::new(),
                items: None,
                variants,
                enum_values: Vec::new(),
                const_value: None,
                constraints: SchemaConstraints::default(),
                nullable,
                confidence: Confidence::Exact,
                evidence: schema_evidence(source),
            });
            return Some(id);
        }
        if let Some(inner) = generic_inner(annotation, &["Literal"]) {
            let values = split_top_level(inner)
                .into_iter()
                .take(MAX_SCHEMA_MEMBERS)
                .filter_map(parse_literal)
                .collect::<Vec<_>>();
            let kind = values
                .first()
                .map_or(SchemaKind::Unknown, |value| literal_kind(value.kind));
            self.push_schema(ApiSchema {
                id: id.clone(),
                name,
                kind,
                format: None,
                properties: Vec::new(),
                items: None,
                variants: Vec::new(),
                enum_values: values,
                const_value: None,
                constraints: SchemaConstraints::default(),
                nullable,
                confidence: Confidence::Exact,
                evidence: schema_evidence(source),
            });
            return Some(id);
        }
        if let Some((kind, format)) = primitive_type(annotation) {
            self.push_schema(ApiSchema {
                id: id.clone(),
                name,
                kind,
                format,
                properties: Vec::new(),
                items: None,
                variants: Vec::new(),
                enum_values: Vec::new(),
                const_value: None,
                constraints: SchemaConstraints::default(),
                nullable,
                confidence: Confidence::Exact,
                evidence: schema_evidence(source),
            });
            return Some(id);
        }
        let model_name = annotation
            .split('.')
            .next_back()
            .unwrap_or(annotation)
            .trim();
        let Some((model_file, region, line)) = resolve_model(file, model_name, self.index) else {
            self.building.remove(&id);
            return None;
        };
        self.building.remove(&id);
        let id = schema_id(&format!("pydantic:{}:{model_name}", model_file.relative));
        if self.known.contains(&id) || self.building.contains(&id) {
            return Some(id);
        }
        self.building.insert(id.clone());
        let model_source = Some(SourceRef {
            file: model_file.relative.clone(),
            line,
            column: 1,
        });
        let mut properties = Vec::new();
        for field in model_fields(region).into_iter().take(MAX_SCHEMA_MEMBERS) {
            let field_origin = format!(
                "pydantic:{}:{model_name}:{}",
                model_file.relative, field.name
            );
            if let Some(field_schema) = self.collect_type(
                model_file,
                annotated_base(&field.annotation),
                &field_origin,
                Some(field.name.clone()),
                model_source.clone(),
            ) {
                let required = field.required();
                properties.push(SchemaProperty {
                    name: field.name,
                    schema_id: field_schema,
                    required,
                });
            }
        }
        let has_validators = region.contains("@field_validator")
            || region.contains("@model_validator")
            || region.contains("@validator");
        if has_validators {
            self.diagnostics.push(Diagnostic {
                code: "schema-runtime-validator".to_owned(),
                severity: DiagnosticSeverity::Warning,
                message: format!("Pydantic runtime validators for {model_name} are not simulated"),
                source: model_source.clone(),
            });
        }
        self.push_schema(ApiSchema {
            id: id.clone(),
            name: Some(model_name.to_owned()),
            kind: SchemaKind::Object,
            format: None,
            properties,
            items: None,
            variants: Vec::new(),
            enum_values: Vec::new(),
            const_value: None,
            constraints: SchemaConstraints::default(),
            nullable,
            confidence: if has_validators {
                Confidence::Inferred
            } else {
                Confidence::Exact
            },
            evidence: schema_evidence(model_source),
        });
        Some(id)
    }

    fn push_schema(&mut self, schema: ApiSchema) {
        self.building.remove(&schema.id);
        self.known.insert(schema.id.clone());
        self.schemas.push(schema);
    }

    fn report_budget(&mut self, source: Option<SourceRef>) {
        if self.budget_reported {
            return;
        }
        self.budget_reported = true;
        self.diagnostics.push(Diagnostic {
            code: "schema-analysis-budget".to_owned(),
            severity: DiagnosticSeverity::Warning,
            message:
                "Stopped expanding a Python schema after reaching the per-route analysis budget"
                    .to_owned(),
            source,
        });
    }
}

struct ModelField {
    name: String,
    annotation: String,
    default: Option<String>,
}

impl ModelField {
    fn required(&self) -> bool {
        self.default
            .as_deref()
            .is_none_or(|value| value.trim() == "..." || value.contains("Field(..."))
            && !self.annotation.contains("Optional[")
            && !self.annotation.contains("| None")
    }
}

fn model_fields(region: &str) -> Vec<ModelField> {
    static FIELD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^[ \t]+([A-Za-z_]\w*)\s*:\s*([^=\n]+?)(?:\s*=\s*([^\n#]+))?$")
            .expect("valid Pydantic field regex")
    });
    FIELD
        .captures_iter(region)
        .filter_map(|captures| {
            let name = captures.get(1)?.as_str();
            if name.starts_with('_') || name == "model_config" {
                return None;
            }
            Some(ModelField {
                name: name.to_owned(),
                annotation: captures.get(2)?.as_str().trim().to_owned(),
                default: captures
                    .get(3)
                    .map(|value| value.as_str().trim().to_owned()),
            })
        })
        .collect()
}

fn resolve_model<'a>(
    file: &'a PythonFile,
    model_name: &str,
    index: &'a PythonIndex,
) -> Option<(&'a PythonFile, &'a str, u32)> {
    if let Some((region, line)) = model_region(&file.source, model_name) {
        return Some((file, region, line));
    }
    let target = file
        .imports
        .iter()
        .find(|import| import.locals.iter().any(|local| local == model_name))?
        .resolved
        .as_ref()
        .and_then(|path| index.get(path))?;
    let (region, line) = model_region(&target.source, model_name)?;
    Some((target, region, line))
}

fn model_region<'a>(source: &'a str, model_name: &str) -> Option<(&'a str, u32)> {
    let declaration = Regex::new(&format!(
        r"(?m)^class\s+{}\s*\([^\n)]*BaseModel[^\n)]*\)\s*:",
        regex::escape(model_name)
    ))
    .ok()?
    .find(source)?;
    let line = u32::try_from(
        source[..declaration.start()]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            + 1,
    )
    .unwrap_or(u32::MAX);
    let tail = &source[declaration.end()..];
    let end = Regex::new(r"(?m)^(?:class|def|async\s+def)\s+")
        .expect("valid Python top-level declaration regex")
        .find(tail)
        .map_or(source.len(), |next| declaration.end() + next.start());
    source
        .get(declaration.start()..end)
        .map(|region| (region, line))
}

fn is_model_type(file: &PythonFile, annotation: &str, index: &PythonIndex) -> bool {
    let annotation = optional_inner(annotated_base(annotation)).0;
    let model_name = annotation
        .split('.')
        .next_back()
        .unwrap_or(annotation)
        .trim();
    resolve_model(file, model_name, index).is_some()
}

fn primitive_type(annotation: &str) -> Option<(SchemaKind, Option<String>)> {
    match annotation.trim() {
        "str" | "StrictStr" => Some((SchemaKind::String, None)),
        "EmailStr" => Some((SchemaKind::String, Some("email".to_owned()))),
        "UUID" | "UUID4" => Some((SchemaKind::String, Some("uuid".to_owned()))),
        "datetime" => Some((SchemaKind::String, Some("date-time".to_owned()))),
        "date" => Some((SchemaKind::String, Some("date".to_owned()))),
        "HttpUrl" | "AnyUrl" => Some((SchemaKind::String, Some("uri".to_owned()))),
        "int" | "StrictInt" => Some((SchemaKind::Integer, None)),
        "float" | "Decimal" => Some((SchemaKind::Number, None)),
        "bool" | "StrictBool" => Some((SchemaKind::Boolean, None)),
        _ => None,
    }
}

fn optional_inner(annotation: &str) -> (&str, bool) {
    if let Some(inner) = generic_inner(annotation, &["Optional"]) {
        return (inner.trim(), true);
    }
    if let Some((left, right)) = annotation.rsplit_once('|')
        && right.trim() == "None"
    {
        return (left.trim(), true);
    }
    (annotation, false)
}

fn annotated_base(annotation: &str) -> &str {
    generic_inner(annotation.trim(), &["Annotated"])
        .and_then(|inner| split_top_level(inner).first().copied())
        .unwrap_or(annotation.trim())
}

fn generic_inner<'a>(annotation: &'a str, names: &[&str]) -> Option<&'a str> {
    let annotation = annotation.trim();
    let open = annotation.find('[')?;
    let name = annotation[..open].trim().split('.').next_back()?;
    if !names.contains(&name) || !annotation.ends_with(']') {
        return None;
    }
    annotation.get(open + 1..annotation.len().saturating_sub(1))
}

fn decorator_before(source: &str, function_start: usize) -> Option<String> {
    let before = source.get(..function_start)?;
    let start = Regex::new(r"(?m)^[ \t]*@")
        .ok()?
        .find_iter(before)
        .last()?
        .start();
    Some(before[start..].trim().to_owned())
}

fn named_decorator_argument<'a>(decorator: &'a str, name: &str) -> Option<&'a str> {
    let open = decorator.find('(')?;
    let (body, _) = call_body(decorator, open)?;
    split_top_level(body).into_iter().find_map(|argument| {
        let equals = top_level_delimiter(argument, b'=')?;
        (argument[..equals].trim() == name).then(|| argument[equals + 1..].trim())
    })
}

fn status_code(value: &str) -> Option<String> {
    static CODE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b([1-5][0-9]{2})\b").expect("valid status code regex"));
    CODE.captures(value)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_owned())
}

fn is_ignored_response_type(annotation: &str) -> bool {
    matches!(
        annotation.trim(),
        "None" | "Response" | "JSONResponse" | "StreamingResponse" | "Any" | "dict" | "Dict"
    )
}

fn schema_evidence(source: Option<SourceRef>) -> Vec<api_subway_core::Evidence> {
    vec![source_evidence(
        EvidenceKind::Framework,
        "Schema resolved from a FastAPI or Pydantic type annotation",
        source,
    )]
}

fn parse_literal(value: &str) -> Option<SchemaLiteral> {
    let value = value.trim();
    if let Some(value) = parse_quoted(value) {
        return Some(SchemaLiteral {
            kind: LiteralKind::String,
            value,
        });
    }
    if matches!(value, "True" | "False") {
        return Some(SchemaLiteral {
            kind: LiteralKind::Boolean,
            value: value.to_ascii_lowercase(),
        });
    }
    value.parse::<i64>().ok().map(|_| SchemaLiteral {
        kind: LiteralKind::Integer,
        value: value.to_owned(),
    })
}

fn parse_quoted(value: &str) -> Option<String> {
    if value.len() < 2 {
        return None;
    }
    let quote = value.as_bytes().first().copied()?;
    if !matches!(quote, b'\'' | b'"') || value.as_bytes().last().copied() != Some(quote) {
        return None;
    }
    Some(value[1..value.len().saturating_sub(1)].to_owned())
}

fn literal_kind(kind: LiteralKind) -> SchemaKind {
    match kind {
        LiteralKind::String => SchemaKind::String,
        LiteralKind::Integer => SchemaKind::Integer,
        LiteralKind::Number => SchemaKind::Number,
        LiteralKind::Boolean => SchemaKind::Boolean,
        LiteralKind::Null => SchemaKind::Null,
    }
}

fn call_body(source: &str, open: usize) -> Option<(&str, usize)> {
    let bytes = source.as_bytes();
    if bytes.get(open) != Some(&b'(') {
        return None;
    }
    let mut depth = 0_u32;
    let mut quote = None;
    let mut escaped = false;
    for index in open..bytes.len() {
        let byte = bytes[index];
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == active {
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
                    return Some((&source[open + 1..index], index));
                }
            }
            _ => {}
        }
    }
    None
}

fn split_top_level(body: &str) -> Vec<&str> {
    let mut output = Vec::new();
    let mut start = 0;
    let mut nesting = 0_i32;
    let mut quote = None;
    let mut escaped = false;
    for (index, byte) in body.bytes().enumerate() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == active {
                quote = None;
            }
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
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

fn top_level_delimiter(value: &str, delimiter: u8) -> Option<usize> {
    let mut nesting = 0_i32;
    let mut quote = None;
    let mut escaped = false;
    for (index, byte) in value.bytes().enumerate() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == active {
                quote = None;
            }
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'(' | b'[' | b'{' => nesting += 1,
            b')' | b']' | b'}' => nesting -= 1,
            byte if byte == delimiter && nesting == 0 => return Some(index),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{annotated_base, model_fields, parse_parameter, parse_quoted};

    #[test]
    fn parses_fastapi_parameters_and_pydantic_fields() {
        let parameter =
            parse_parameter("page: Annotated[int, Query(ge=1)] = 1").expect("parameter");
        assert_eq!(parameter.name, "page");
        assert_eq!(annotated_base(&parameter.annotation), "int");
        let fields = model_fields(
            "class User(BaseModel):\n    name: str\n    role: Literal['admin', 'member'] = 'member'\n",
        );
        assert_eq!(fields.len(), 2);
        assert!(fields[0].required());
        assert!(!fields[1].required());
    }

    #[test]
    fn a_single_quote_is_not_a_string_literal() {
        assert_eq!(parse_quoted("'"), None);
        assert_eq!(parse_quoted("\""), None);
    }
}
