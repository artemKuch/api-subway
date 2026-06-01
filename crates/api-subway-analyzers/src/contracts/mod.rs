pub(crate) mod openapi;
pub(crate) mod python;
pub(crate) mod zod;

use std::sync::LazyLock;

use api_subway_core::{
    ApiSchema, Confidence, Diagnostic, Endpoint, EndpointContract, Evidence, EvidenceKind,
    ParameterContract, ParameterLocation, RequestContract, SchemaConstraints, SchemaKind,
    SourceRef, schema_id,
};
use regex::Regex;

#[derive(Debug, Default)]
pub(crate) struct ContractAnalysis {
    pub contract: Option<EndpointContract>,
    pub schemas: Vec<ApiSchema>,
    pub diagnostics: Vec<Diagnostic>,
}

pub(crate) fn add_inferred_path_parameters(endpoint: &mut Endpoint, schemas: &mut Vec<ApiSchema>) {
    static PARAMETER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\{([^}?*]+)([?*]*)\}").expect("valid normalized path parameter regex")
    });
    let source = endpoint.sources.first().cloned();
    let mut parameters = Vec::new();
    let mut inferred_schemas = Vec::new();
    for captures in PARAMETER.captures_iter(&endpoint.path) {
        let Some(name) = captures.get(1).map(|value| value.as_str().to_owned()) else {
            continue;
        };
        let required = !captures
            .get(2)
            .is_some_and(|markers| markers.as_str().contains('?'));
        let id = schema_id(&format!("{}:path:{name}", endpoint.id));
        parameters.push(ParameterContract {
            name: name.clone(),
            location: ParameterLocation::Path,
            required,
            schema_id: id.clone(),
        });
        inferred_schemas.push(ApiSchema {
            id,
            name: Some(name),
            kind: SchemaKind::String,
            format: None,
            properties: Vec::new(),
            items: None,
            variants: Vec::new(),
            enum_values: Vec::new(),
            const_value: None,
            constraints: SchemaConstraints::default(),
            nullable: false,
            confidence: Confidence::Inferred,
            evidence: vec![Evidence {
                kind: EvidenceKind::Heuristic,
                detail: "Inferred from a normalized route path parameter".to_owned(),
                source: source.clone(),
            }],
        });
    }
    if parameters.is_empty() {
        return;
    }
    schemas.extend(inferred_schemas);
    let contract = endpoint.contract.get_or_insert_with(|| EndpointContract {
        confidence: Confidence::Inferred,
        request: RequestContract::default(),
        responses: Vec::new(),
        evidence: Vec::new(),
    });
    for parameter in parameters {
        if !contract
            .request
            .parameters
            .iter()
            .any(|current| current.name == parameter.name && current.location == parameter.location)
        {
            contract.request.parameters.push(parameter);
            contract.confidence = Confidence::Inferred;
        }
    }
    contract.evidence.push(Evidence {
        kind: EvidenceKind::Heuristic,
        detail: "Route path exposes one or more required parameters".to_owned(),
        source,
    });
}

pub(crate) fn source_evidence(
    kind: EvidenceKind,
    detail: impl Into<String>,
    source: Option<SourceRef>,
) -> Evidence {
    Evidence {
        kind,
        detail: detail.into(),
        source,
    }
}
