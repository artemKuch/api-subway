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
    javascript::{JsFile, JsIndex},
};

use super::{ContractAnalysis, source_evidence};

const MAX_SCHEMAS_PER_ROUTE: usize = 1_000;
const MAX_SCHEMA_RECURSION: usize = 64;
const MAX_SCHEMA_MEMBERS: usize = 1_000;
const MAX_SCHEMA_WORK: usize = 10_000;

pub(crate) fn analyze_route(route: &RouteRecord, index: &JsIndex) -> ContractAnalysis {
    let Some(file) = index.get(&route.source_path) else {
        return ContractAnalysis::default();
    };
    let scopes = route
        .entry_symbols
        .iter()
        .filter_map(|symbol| file.scope_for_symbol(symbol))
        .chain(route.inline_code.iter().cloned())
        .collect::<Vec<_>>();
    if scopes.is_empty() {
        return ContractAnalysis::default();
    }
    let scope = scopes.join("\n");
    let mut collector = Collector {
        index,
        schemas: Vec::new(),
        known: BTreeSet::new(),
        building: BTreeSet::new(),
        diagnostics: Vec::new(),
        work: 0,
        budget_reported: false,
    };
    let source = route.endpoint.sources.first().cloned();
    let mut parameters = Vec::new();
    let mut bodies = Vec::new();
    let mut responses = Vec::new();
    let mut evidence = Vec::new();
    let mut confidence = Confidence::Exact;
    for usage in schema_usages(&scope) {
        let Some((schema_file, expression)) = resolve_schema_expression(file, &usage.symbol, index)
        else {
            continue;
        };
        let origin = format!("zod:{}:{}", schema_file.relative, usage.symbol);
        let schema = collector.collect_expression(
            schema_file,
            &expression,
            &origin,
            Some(usage.symbol.clone()),
            source.clone(),
        );
        if expression_has_runtime_transform(&expression) {
            confidence = Confidence::Inferred;
            collector.diagnostics.push(Diagnostic {
                code: "schema-runtime-transform".to_owned(),
                severity: DiagnosticSeverity::Warning,
                message: format!(
                    "Zod runtime transforms for {} are not simulated",
                    usage.symbol
                ),
                source: source.clone(),
            });
        }
        let Some(schema_id) = schema else {
            continue;
        };
        let item_evidence = source_evidence(
            EvidenceKind::Call,
            format!(
                "Handler validates data with {}.{}",
                usage.symbol, usage.method
            ),
            source.clone(),
        );
        match usage.channel {
            UsageChannel::Body => {
                insert_unique(
                    &mut bodies,
                    ContentContract {
                        media_type: "application/json".to_owned(),
                        schema_id,
                        required: true,
                    },
                );
                evidence.push(item_evidence);
            }
            UsageChannel::Query | UsageChannel::Path => {
                let location = if usage.channel == UsageChannel::Path {
                    ParameterLocation::Path
                } else {
                    ParameterLocation::Query
                };
                if let Some(root) = collector.schemas.iter().find(|item| item.id == schema_id) {
                    for property in &root.properties {
                        insert_unique(
                            &mut parameters,
                            ParameterContract {
                                name: property.name.clone(),
                                location,
                                required: property.required || location == ParameterLocation::Path,
                                schema_id: property.schema_id.clone(),
                            },
                        );
                    }
                    evidence.push(item_evidence);
                }
            }
            UsageChannel::Response => {
                let status = response_status(&scope, usage.offset);
                insert_unique(
                    &mut responses,
                    ResponseContract {
                        status,
                        contents: vec![ContentContract {
                            media_type: "application/json".to_owned(),
                            schema_id,
                            required: false,
                        }],
                    },
                );
                evidence.push(item_evidence);
            }
            UsageChannel::Unknown => {}
        }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UsageChannel {
    Body,
    Query,
    Path,
    Response,
    Unknown,
}

struct SchemaUsage {
    symbol: String,
    method: String,
    channel: UsageChannel,
    offset: usize,
}

fn schema_usages(scope: &str) -> Vec<SchemaUsage> {
    static PARSE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\b([A-Za-z_$][\w$]*)\s*\.\s*(parse|safeParse)\s*\(")
            .expect("valid Zod parse call regex")
    });
    PARSE
        .captures_iter(scope)
        .filter_map(|captures| {
            let whole = captures.get(0)?;
            let open = whole.end() - 1;
            let (argument, _) = call_body(scope, open)?;
            let before = &scope[..whole.start()];
            let channel = if response_wrapper_is_open(before) {
                UsageChannel::Response
            } else if argument.contains(".body") || argument.contains(".json(") {
                UsageChannel::Body
            } else if argument.contains(".query") || argument.contains("searchParams") {
                UsageChannel::Query
            } else if argument.contains(".params") {
                UsageChannel::Path
            } else {
                UsageChannel::Unknown
            };
            Some(SchemaUsage {
                symbol: captures.get(1)?.as_str().to_owned(),
                method: captures.get(2)?.as_str().to_owned(),
                channel,
                offset: whole.start(),
            })
        })
        .collect()
}

fn response_wrapper_is_open(source: &str) -> bool {
    let tail = source
        .rsplit([';', '\n'])
        .next()
        .unwrap_or(source)
        .trim_start();
    tail.contains(".json(")
}

fn response_status(scope: &str, offset: usize) -> String {
    static STATUS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\.status\s*\(\s*([1-5][0-9]{2})\s*\)").expect("valid response status regex")
    });
    static OPTION_STATUS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\bstatus\s*:\s*([1-5][0-9]{2})\b").expect("valid response option status regex")
    });
    let start = char_boundary_at_or_before(scope, offset.saturating_sub(160));
    let end = char_boundary_at_or_before(scope, offset.saturating_add(240).min(scope.len()));
    STATUS
        .captures(&scope[start..offset])
        .and_then(|captures| captures.get(1))
        .or_else(|| {
            OPTION_STATUS
                .captures(&scope[start..end])
                .and_then(|captures| captures.get(1))
        })
        .map_or_else(|| "200".to_owned(), |value| value.as_str().to_owned())
}

fn char_boundary_at_or_before(value: &str, mut offset: usize) -> usize {
    offset = offset.min(value.len());
    while offset > 0 && !value.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn resolve_schema_expression<'a>(
    file: &'a JsFile,
    symbol: &str,
    index: &'a JsIndex,
) -> Option<(&'a JsFile, String)> {
    if let Some(expression) = declaration_expression(&file.source, symbol) {
        return Some((file, expression));
    }
    let resolved = file
        .imports
        .iter()
        .find(|import| import.locals.iter().any(|local| local == symbol))?
        .resolved
        .as_ref()?;
    let target = index.get(resolved)?;
    declaration_expression(&target.source, symbol).map(|expression| (target, expression))
}

fn declaration_expression(source: &str, symbol: &str) -> Option<String> {
    let declaration = Regex::new(&format!(
        r"(?m)\b(?:export\s+)?const\s+{}\s*=\s*",
        regex::escape(symbol)
    ))
    .ok()?
    .find(source)?;
    expression_at(source, declaration.end())
        .map(str::trim)
        .map(str::to_owned)
}

fn expression_at(source: &str, start: usize) -> Option<&str> {
    let bytes = source.as_bytes();
    let mut nesting = 0_i32;
    let mut quote = None;
    let mut escaped = false;
    for (index, &byte) in bytes.iter().enumerate().skip(start) {
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
            b'\'' | b'"' | b'`' => quote = Some(byte),
            b'(' | b'[' | b'{' => nesting += 1,
            b')' | b']' | b'}' => nesting -= 1,
            b';' | b'\n' if nesting == 0 => return source.get(start..index),
            _ => {}
        }
    }
    source.get(start..)
}

struct Collector<'a> {
    index: &'a JsIndex,
    schemas: Vec<ApiSchema>,
    known: BTreeSet<String>,
    building: BTreeSet<String>,
    diagnostics: Vec<Diagnostic>,
    work: usize,
    budget_reported: bool,
}

impl Collector<'_> {
    fn collect_expression(
        &mut self,
        file: &JsFile,
        expression: &str,
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
        let id = schema_id(origin);
        if self.known.contains(&id) || self.building.contains(&id) {
            return Some(id);
        }
        self.building.insert(id.clone());
        let expression = expression.trim();
        if is_identifier(expression)
            && let Some((target, target_expression)) =
                resolve_schema_expression(file, expression, self.index)
        {
            let resolved = self.collect_expression(
                target,
                &target_expression,
                &format!("zod:{}:{expression}", target.relative),
                Some(expression.to_owned()),
                source,
            );
            self.building.remove(&id);
            return resolved;
        }
        let mut kind = SchemaKind::Unknown;
        let mut format = None;
        let mut properties = Vec::new();
        let mut items = None;
        let mut variants = Vec::new();
        let mut enum_values = Vec::new();
        let mut const_value = None;
        if let Some(body) = zod_call_body(expression, "object") {
            kind = SchemaKind::Object;
            let object = body.trim().strip_prefix('{')?.strip_suffix('}')?;
            for property in split_top_level(object).into_iter().take(MAX_SCHEMA_MEMBERS) {
                let Some((raw_name, property_expression)) = split_property(property) else {
                    continue;
                };
                let property_name = parse_property_name(raw_name)?;
                let property_origin = format!("{origin}:property:{property_name}");
                let property_schema = self.collect_expression(
                    file,
                    property_expression,
                    &property_origin,
                    Some(property_name.clone()),
                    source.clone(),
                )?;
                properties.push(SchemaProperty {
                    name: property_name,
                    schema_id: property_schema,
                    required: !is_optional(property_expression),
                });
            }
        } else if let Some(body) = zod_call_body(expression, "array") {
            kind = SchemaKind::Array;
            items = self.collect_expression(
                file,
                split_top_level(body).first().copied().unwrap_or(""),
                &format!("{origin}:items"),
                None,
                source.clone(),
            );
        } else if let Some(body) = zod_call_body(expression, "union") {
            kind = SchemaKind::Union;
            let values = body.trim().strip_prefix('[')?.strip_suffix(']')?;
            for (index, variant) in split_top_level(values)
                .into_iter()
                .take(MAX_SCHEMA_MEMBERS)
                .enumerate()
            {
                if let Some(variant) = self.collect_expression(
                    file,
                    variant,
                    &format!("{origin}:variant:{index}"),
                    None,
                    source.clone(),
                ) {
                    variants.push(variant);
                }
            }
        } else if let Some(body) = zod_call_body(expression, "enum") {
            kind = SchemaKind::String;
            enum_values = quoted_values(body)
                .into_iter()
                .map(|value| SchemaLiteral {
                    kind: LiteralKind::String,
                    value,
                })
                .collect();
        } else if let Some(body) = zod_call_body(expression, "literal") {
            const_value = parse_literal(body.trim());
            kind = const_value
                .as_ref()
                .map_or(SchemaKind::Unknown, |value| literal_schema_kind(value.kind));
        } else if expression.contains("z.string(") {
            kind = SchemaKind::String;
            format = string_format(expression);
        } else if expression.contains("z.number(") {
            kind = if expression.contains(".int(") {
                SchemaKind::Integer
            } else {
                SchemaKind::Number
            };
        } else if expression.contains("z.boolean(") {
            kind = SchemaKind::Boolean;
        }
        let confidence = if expression_has_runtime_transform(expression) {
            Confidence::Inferred
        } else {
            Confidence::Exact
        };
        let schema = ApiSchema {
            id: id.clone(),
            name,
            kind,
            format,
            properties,
            items,
            variants,
            enum_values,
            const_value,
            constraints: zod_constraints(expression, kind),
            nullable: expression.contains(".nullable(") || expression.starts_with("z.nullable("),
            confidence,
            evidence: vec![source_evidence(
                EvidenceKind::Call,
                "Schema resolved from a Zod declaration used by the handler",
                source,
            )],
        };
        self.schemas.push(schema);
        self.building.remove(&id);
        self.known.insert(id.clone());
        Some(id)
    }

    fn report_budget(&mut self, source: Option<SourceRef>) {
        if self.budget_reported {
            return;
        }
        self.budget_reported = true;
        self.diagnostics.push(Diagnostic {
            code: "schema-analysis-budget".to_owned(),
            severity: DiagnosticSeverity::Warning,
            message: "Stopped expanding a Zod schema after reaching the per-route analysis budget"
                .to_owned(),
            source,
        });
    }
}

fn zod_call_body<'a>(expression: &'a str, method: &str) -> Option<&'a str> {
    let pattern = format!("z.{method}");
    let start = expression.find(&pattern)? + pattern.len();
    let open = expression[start..].find('(')? + start;
    call_body(expression, open).map(|(body, _)| body)
}

fn zod_constraints(expression: &str, kind: SchemaKind) -> SchemaConstraints {
    static MIN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\.min\s*\(\s*([0-9]+(?:\.[0-9]+)?)").expect("valid Zod min regex")
    });
    static MAX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\.max\s*\(\s*([0-9]+(?:\.[0-9]+)?)").expect("valid Zod max regex")
    });
    let min = MIN
        .captures(expression)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_owned());
    let max = MAX
        .captures(expression)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_owned());
    let min_count = min.as_deref().and_then(|value| value.parse().ok());
    let max_count = max.as_deref().and_then(|value| value.parse().ok());
    SchemaConstraints {
        min_length: (kind == SchemaKind::String).then_some(min_count).flatten(),
        max_length: (kind == SchemaKind::String).then_some(max_count).flatten(),
        minimum: matches!(kind, SchemaKind::Integer | SchemaKind::Number)
            .then_some(min.clone())
            .flatten(),
        maximum: matches!(kind, SchemaKind::Integer | SchemaKind::Number)
            .then_some(max.clone())
            .flatten(),
        min_items: (kind == SchemaKind::Array).then_some(min_count).flatten(),
        max_items: (kind == SchemaKind::Array).then_some(max_count).flatten(),
        pattern: None,
    }
}

fn string_format(expression: &str) -> Option<String> {
    ["email", "uuid", "datetime", "date", "url"]
        .into_iter()
        .find(|format| expression.contains(&format!(".{format}(")))
        .map(|format| match format {
            "datetime" => "date-time".to_owned(),
            "url" => "uri".to_owned(),
            value => value.to_owned(),
        })
}

fn expression_has_runtime_transform(expression: &str) -> bool {
    [".transform(", ".refine(", ".superRefine(", ".preprocess("]
        .iter()
        .any(|marker| expression.contains(marker))
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

fn split_property(value: &str) -> Option<(&str, &str)> {
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
            b'\'' | b'"' | b'`' => quote = Some(byte),
            b'(' | b'[' | b'{' => nesting += 1,
            b')' | b']' | b'}' => nesting -= 1,
            b':' if nesting == 0 => return Some((&value[..index], &value[index + 1..])),
            _ => {}
        }
    }
    None
}

fn parse_property_name(value: &str) -> Option<String> {
    let value = value.trim();
    if is_identifier(value) {
        return Some(value.to_owned());
    }
    parse_quoted(value)
}

fn quoted_values(value: &str) -> Vec<String> {
    let value = value.trim();
    let values = value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'));
    values
        .map(split_top_level)
        .unwrap_or_default()
        .into_iter()
        .filter_map(parse_quoted)
        .collect()
}

fn parse_literal(value: &str) -> Option<SchemaLiteral> {
    if let Some(value) = parse_quoted(value) {
        return Some(SchemaLiteral {
            kind: LiteralKind::String,
            value,
        });
    }
    if matches!(value, "true" | "false") {
        return Some(SchemaLiteral {
            kind: LiteralKind::Boolean,
            value: value.to_owned(),
        });
    }
    if value == "null" {
        return Some(SchemaLiteral {
            kind: LiteralKind::Null,
            value: "null".to_owned(),
        });
    }
    value.parse::<i64>().ok().map(|_| SchemaLiteral {
        kind: LiteralKind::Integer,
        value: value.to_owned(),
    })
}

fn parse_quoted(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() < 2 {
        return None;
    }
    let quote = value.as_bytes().first().copied()?;
    if !matches!(quote, b'\'' | b'"' | b'`') || value.as_bytes().last().copied() != Some(quote) {
        return None;
    }
    Some(value[1..value.len().saturating_sub(1)].to_owned())
}

fn literal_schema_kind(kind: LiteralKind) -> SchemaKind {
    match kind {
        LiteralKind::String => SchemaKind::String,
        LiteralKind::Integer => SchemaKind::Integer,
        LiteralKind::Number => SchemaKind::Number,
        LiteralKind::Boolean => SchemaKind::Boolean,
        LiteralKind::Null => SchemaKind::Null,
    }
}

fn is_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    characters.next().is_some_and(|character| {
        character == '_' || character == '$' || character.is_ascii_alphabetic()
    }) && characters
        .all(|character| character == '_' || character == '$' || character.is_ascii_alphanumeric())
}

fn is_optional(value: &str) -> bool {
    value.contains(".optional(") || value.trim_start().starts_with("z.optional(")
}

fn insert_unique<T: PartialEq>(values: &mut Vec<T>, value: T) {
    if !values.contains(&value) {
        values.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_quoted, quoted_values, response_status, split_top_level, zod_constraints};
    use api_subway_core::SchemaKind;

    #[test]
    fn splits_nested_zod_objects_and_reads_constraints() {
        assert_eq!(
            split_top_level("name: z.string(), role: z.enum(['admin', 'member'])").len(),
            2
        );
        assert_eq!(quoted_values("['admin', 'member']"), ["admin", "member"]);
        let constraints = zod_constraints("z.string().min(2).max(40)", SchemaKind::String);
        assert_eq!(constraints.min_length, Some(2));
        assert_eq!(constraints.max_length, Some(40));
    }

    #[test]
    fn handles_malformed_literals_and_unicode_status_context() {
        assert_eq!(parse_quoted("'"), None);
        let source = format!("{}res.status(201).json(User.parse(value))", "é".repeat(120));
        let offset = source.find("User.parse").expect("parse call");
        assert_eq!(response_status(&source, offset), "201");
    }
}
