use std::collections::{BTreeMap, BTreeSet};

use crate::{
    ApiMapV1, ApiSchema, Confidence, ContentContract, Dependency, Diagnostic, DiagnosticSeverity,
    Endpoint, EndpointContract, ParameterContract, ProjectInfo, Relation, ResponseContract,
    SchemaKind,
};

const MAX_ENDPOINTS: usize = 10_000;
const MAX_SCHEMAS: usize = 50_000;
const MAX_DEPENDENCIES: usize = 10_000;
const MAX_RELATIONS: usize = 100_000;
const MAX_DIAGNOSTICS: usize = 50_000;
const MAX_NESTED_ITEMS: usize = 1_000;
const MAX_FRAMEWORKS: usize = 64;

pub struct ApiMapBuilder {
    project_name: String,
    frameworks: BTreeSet<String>,
    endpoints: BTreeMap<String, Endpoint>,
    schemas: BTreeMap<String, ApiSchema>,
    dependencies: BTreeMap<String, Dependency>,
    relations: BTreeMap<(String, String), Relation>,
    diagnostics: BTreeSet<Diagnostic>,
}

impl ApiMapBuilder {
    pub fn new(project_name: impl Into<String>) -> Self {
        Self {
            project_name: project_name.into(),
            frameworks: BTreeSet::new(),
            endpoints: BTreeMap::new(),
            schemas: BTreeMap::new(),
            dependencies: BTreeMap::new(),
            relations: BTreeMap::new(),
            diagnostics: BTreeSet::new(),
        }
    }

    pub fn add_framework(&mut self, framework: impl Into<String>) {
        let framework = framework.into();
        if self.frameworks.contains(&framework) {
            return;
        }
        if self.frameworks.len() < MAX_FRAMEWORKS {
            self.frameworks.insert(framework);
        } else {
            self.add_budget_diagnostic("frameworks", MAX_FRAMEWORKS);
        }
    }

    pub fn add_endpoint(&mut self, mut endpoint: Endpoint) {
        let mut truncated = bound_endpoint(&mut endpoint);
        if let Some(current) = self.endpoints.get_mut(&endpoint.id) {
            truncated |= merge_unique_bounded(&mut current.tags, &endpoint.tags, MAX_NESTED_ITEMS);
            truncated |=
                merge_unique_bounded(&mut current.sources, &endpoint.sources, MAX_NESTED_ITEMS);
            current.spec_only &= endpoint.spec_only;
            if current.operation_id.is_none() {
                current.operation_id.clone_from(&endpoint.operation_id);
            }
            if current.framework == "openapi" && endpoint.framework != "openapi" {
                current.framework.clone_from(&endpoint.framework);
                current.display_path.clone_from(&endpoint.display_path);
            }
            truncated |= merge_contract(&mut current.contract, endpoint.contract.as_ref());
        } else if self.endpoints.len() < MAX_ENDPOINTS {
            self.endpoints.insert(endpoint.id.clone(), endpoint);
        } else {
            self.add_budget_diagnostic("endpoints", MAX_ENDPOINTS);
            return;
        }
        if truncated {
            self.add_budget_diagnostic("endpoint nested items", MAX_NESTED_ITEMS);
        }
    }

    pub fn add_schema(&mut self, mut schema: ApiSchema) {
        let mut truncated = bound_schema(&mut schema);
        if let Some(current) = self.schemas.get_mut(&schema.id) {
            if schema.confidence == Confidence::Exact {
                current.confidence = Confidence::Exact;
            }
            if current.kind == SchemaKind::Unknown && schema.kind != SchemaKind::Unknown {
                current.kind = schema.kind;
                current.format.clone_from(&schema.format);
                current.items.clone_from(&schema.items);
                current.const_value.clone_from(&schema.const_value);
                current.constraints.clone_from(&schema.constraints);
                current.nullable = schema.nullable;
            }
            if current.name.is_none() {
                current.name.clone_from(&schema.name);
            }
            truncated |= merge_unique_bounded(
                &mut current.properties,
                &schema.properties,
                MAX_NESTED_ITEMS,
            );
            truncated |=
                merge_unique_bounded(&mut current.variants, &schema.variants, MAX_NESTED_ITEMS);
            truncated |= merge_unique_bounded(
                &mut current.enum_values,
                &schema.enum_values,
                MAX_NESTED_ITEMS,
            );
            truncated |=
                merge_unique_bounded(&mut current.evidence, &schema.evidence, MAX_NESTED_ITEMS);
        } else if self.schemas.len() < MAX_SCHEMAS {
            self.schemas.insert(schema.id.clone(), schema);
        } else {
            self.add_budget_diagnostic("schemas", MAX_SCHEMAS);
            return;
        }
        if truncated {
            self.add_budget_diagnostic("schema nested items", MAX_NESTED_ITEMS);
        }
    }

    pub fn add_dependency(&mut self, mut dependency: Dependency) {
        let mut truncated = sort_dedup_truncate(&mut dependency.packages, MAX_NESTED_ITEMS);
        if let Some(current) = self.dependencies.get_mut(&dependency.id) {
            current.pinned |= dependency.pinned;
            truncated |= merge_unique_bounded(
                &mut current.packages,
                &dependency.packages,
                MAX_NESTED_ITEMS,
            );
        } else if self.dependencies.len() < MAX_DEPENDENCIES {
            self.dependencies.insert(dependency.id.clone(), dependency);
        } else {
            self.add_budget_diagnostic("dependencies", MAX_DEPENDENCIES);
            return;
        }
        if truncated {
            self.add_budget_diagnostic("dependency packages", MAX_NESTED_ITEMS);
        }
    }

    pub fn add_relation(&mut self, mut relation: Relation) {
        let mut truncated = sort_dedup_truncate(&mut relation.evidence, MAX_NESTED_ITEMS);
        let key = (relation.endpoint_id.clone(), relation.dependency_id.clone());
        if let Some(current) = self.relations.get_mut(&key) {
            if relation.confidence == Confidence::Exact {
                current.confidence = Confidence::Exact;
            }
            truncated |=
                merge_unique_bounded(&mut current.evidence, &relation.evidence, MAX_NESTED_ITEMS);
        } else if self.relations.len() < MAX_RELATIONS {
            self.relations.insert(key, relation);
        } else {
            self.add_budget_diagnostic("relations", MAX_RELATIONS);
            return;
        }
        if truncated {
            self.add_budget_diagnostic("relation evidence", MAX_NESTED_ITEMS);
        }
    }

    pub fn add_diagnostic(&mut self, diagnostic: Diagnostic) {
        if self.diagnostics.len() < MAX_DIAGNOSTICS {
            self.diagnostics.insert(diagnostic);
        }
    }

    fn add_budget_diagnostic(&mut self, scope: &str, maximum: usize) {
        if self.diagnostics.len() >= MAX_DIAGNOSTICS {
            return;
        }
        self.diagnostics.insert(Diagnostic {
            code: "model-budget-truncated".to_owned(),
            severity: DiagnosticSeverity::Warning,
            message: format!("Truncated {scope} at the supported {maximum}-item model budget"),
            source: None,
        });
    }

    pub fn build(mut self) -> ApiMapV1 {
        for endpoint in self.endpoints.values_mut() {
            endpoint.tags.sort();
            endpoint.sources.sort();
            if let Some(contract) = &mut endpoint.contract {
                normalize_contract(contract);
            }
        }
        for schema in self.schemas.values_mut() {
            schema.properties.sort();
            schema.properties.dedup();
            schema.variants.sort();
            schema.variants.dedup();
            schema.enum_values.sort();
            schema.enum_values.dedup();
            schema.evidence.sort();
            schema.evidence.dedup();
        }
        for dependency in self.dependencies.values_mut() {
            dependency.packages.sort();
        }
        for relation in self.relations.values_mut() {
            relation.evidence.sort();
        }

        let mut endpoints = self.endpoints.into_values().collect::<Vec<_>>();
        endpoints.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| {
                    crate::method_rank(&left.method).cmp(&crate::method_rank(&right.method))
                })
                .then_with(|| left.method.cmp(&right.method))
        });
        let reachable_schemas = reachable_schema_ids(&endpoints, &self.schemas);
        let schemas = self
            .schemas
            .into_iter()
            .filter_map(|(id, schema)| reachable_schemas.contains(&id).then_some(schema))
            .collect();

        ApiMapV1 {
            schema_version: ApiMapV1::SCHEMA_VERSION,
            project: ProjectInfo {
                name: self.project_name,
                root: ".".to_owned(),
                frameworks: self.frameworks.into_iter().collect(),
            },
            endpoints,
            schemas,
            dependencies: self.dependencies.into_values().collect(),
            relations: self.relations.into_values().collect(),
            diagnostics: self.diagnostics.into_iter().collect(),
        }
    }
}

fn reachable_schema_ids(
    endpoints: &[Endpoint],
    schemas: &BTreeMap<String, ApiSchema>,
) -> BTreeSet<String> {
    let mut reachable = BTreeSet::new();
    let mut pending = Vec::new();
    for contract in endpoints
        .iter()
        .filter_map(|endpoint| endpoint.contract.as_ref())
    {
        pending.extend(
            contract
                .request
                .parameters
                .iter()
                .map(|parameter| parameter.schema_id.clone()),
        );
        pending.extend(
            contract
                .request
                .bodies
                .iter()
                .map(|content| content.schema_id.clone()),
        );
        pending.extend(
            contract
                .responses
                .iter()
                .flat_map(|response| response.contents.iter())
                .map(|content| content.schema_id.clone()),
        );
    }
    while let Some(id) = pending.pop() {
        if !reachable.insert(id.clone()) {
            continue;
        }
        let Some(schema) = schemas.get(&id) else {
            continue;
        };
        pending.extend(
            schema
                .properties
                .iter()
                .map(|property| property.schema_id.clone()),
        );
        pending.extend(schema.items.iter().cloned());
        pending.extend(schema.variants.iter().cloned());
    }
    reachable
}

fn bound_endpoint(endpoint: &mut Endpoint) -> bool {
    let mut truncated = sort_dedup_truncate(&mut endpoint.tags, MAX_NESTED_ITEMS);
    truncated |= sort_dedup_truncate(&mut endpoint.sources, MAX_NESTED_ITEMS);
    if let Some(contract) = &mut endpoint.contract {
        truncated |= bound_contract(contract);
    }
    truncated
}

fn bound_schema(schema: &mut ApiSchema) -> bool {
    let mut truncated = sort_dedup_truncate(&mut schema.properties, MAX_NESTED_ITEMS);
    truncated |= sort_dedup_truncate(&mut schema.variants, MAX_NESTED_ITEMS);
    truncated |= sort_dedup_truncate(&mut schema.enum_values, MAX_NESTED_ITEMS);
    truncated |= sort_dedup_truncate(&mut schema.evidence, MAX_NESTED_ITEMS);
    truncated
}

fn bound_contract(contract: &mut EndpointContract) -> bool {
    normalize_contract(contract);
    let mut truncated = sort_dedup_truncate(&mut contract.request.parameters, MAX_NESTED_ITEMS);
    truncated |= sort_dedup_truncate(&mut contract.request.bodies, MAX_NESTED_ITEMS);
    truncated |= sort_dedup_truncate(&mut contract.responses, MAX_NESTED_ITEMS);
    truncated |= sort_dedup_truncate(&mut contract.evidence, MAX_NESTED_ITEMS);
    for response in &mut contract.responses {
        truncated |= sort_dedup_truncate(&mut response.contents, MAX_NESTED_ITEMS);
    }
    truncated
}

fn merge_contract(
    current: &mut Option<EndpointContract>,
    incoming: Option<&EndpointContract>,
) -> bool {
    let Some(incoming) = incoming else {
        return false;
    };
    let Some(current) = current else {
        *current = Some(incoming.clone());
        return false;
    };
    let mut truncated = false;
    if incoming.confidence == Confidence::Exact {
        current.confidence = Confidence::Exact;
    }
    truncated |= merge_parameters(
        &mut current.request.parameters,
        &incoming.request.parameters,
        incoming.confidence,
    );
    truncated |= merge_contents(
        &mut current.request.bodies,
        &incoming.request.bodies,
        incoming.confidence,
    );
    truncated |= merge_responses(
        &mut current.responses,
        &incoming.responses,
        incoming.confidence,
    );
    truncated |= merge_unique_bounded(&mut current.evidence, &incoming.evidence, MAX_NESTED_ITEMS);
    truncated
}

fn merge_parameters(
    current: &mut Vec<ParameterContract>,
    incoming: &[ParameterContract],
    confidence: Confidence,
) -> bool {
    let mut truncated = false;
    for parameter in incoming {
        if let Some(existing) = current.iter_mut().find(|existing| {
            existing.name == parameter.name && existing.location == parameter.location
        }) {
            if confidence == Confidence::Exact {
                existing.clone_from(parameter);
            }
        } else if current.len() < MAX_NESTED_ITEMS {
            current.push(parameter.clone());
        } else {
            truncated = true;
        }
    }
    truncated
}

fn merge_contents(
    current: &mut Vec<ContentContract>,
    incoming: &[ContentContract],
    confidence: Confidence,
) -> bool {
    let mut truncated = false;
    for content in incoming {
        if let Some(existing) = current
            .iter_mut()
            .find(|existing| existing.media_type == content.media_type)
        {
            if confidence == Confidence::Exact {
                existing.clone_from(content);
            }
        } else if current.len() < MAX_NESTED_ITEMS {
            current.push(content.clone());
        } else {
            truncated = true;
        }
    }
    truncated
}

fn merge_responses(
    current: &mut Vec<ResponseContract>,
    incoming: &[ResponseContract],
    confidence: Confidence,
) -> bool {
    let mut truncated = false;
    for response in incoming {
        if let Some(existing) = current
            .iter_mut()
            .find(|existing| existing.status == response.status)
        {
            truncated |= merge_contents(&mut existing.contents, &response.contents, confidence);
        } else if current.len() < MAX_NESTED_ITEMS {
            current.push(response.clone());
        } else {
            truncated = true;
        }
    }
    truncated
}

fn normalize_contract(contract: &mut EndpointContract) {
    contract.request.parameters.sort();
    contract.request.parameters.dedup();
    contract.request.bodies.sort();
    contract.request.bodies.dedup();
    contract.responses.sort();
    contract.responses.dedup();
    contract.evidence.sort();
    contract.evidence.dedup();
}

fn merge_unique_bounded<T: Clone + Ord>(
    target: &mut Vec<T>,
    incoming: &[T],
    maximum: usize,
) -> bool {
    target.extend_from_slice(incoming);
    sort_dedup_truncate(target, maximum)
}

fn sort_dedup_truncate<T: Ord>(target: &mut Vec<T>, maximum: usize) -> bool {
    target.sort();
    target.dedup();
    let truncated = target.len() > maximum;
    target.truncate(maximum);
    truncated
}

#[cfg(test)]
mod tests {
    use crate::{
        ApiMapBuilder, ApiSchema, Confidence, Endpoint, EndpointContract, ParameterContract,
        ParameterLocation, RequestContract, SchemaConstraints, SchemaKind, canonical_endpoint_id,
        district_for_path, schema_id,
    };

    #[test]
    fn exact_contracts_override_inferred_fields_and_prune_unused_schemas() {
        let endpoint_id = canonical_endpoint_id("GET", "/users/{id}");
        let inferred_id = schema_id("inferred:id");
        let exact_id = schema_id("exact:id");
        let orphan_id = schema_id("orphan");
        let mut builder = ApiMapBuilder::new("contracts");
        for (id, confidence) in [
            (inferred_id.clone(), Confidence::Inferred),
            (exact_id.clone(), Confidence::Exact),
            (orphan_id.clone(), Confidence::Exact),
        ] {
            builder.add_schema(ApiSchema {
                id,
                name: None,
                kind: SchemaKind::String,
                format: None,
                properties: Vec::new(),
                items: None,
                variants: Vec::new(),
                enum_values: Vec::new(),
                const_value: None,
                constraints: SchemaConstraints::default(),
                nullable: false,
                confidence,
                evidence: Vec::new(),
            });
        }
        builder.add_endpoint(endpoint(&endpoint_id, Confidence::Inferred, &inferred_id));
        builder.add_endpoint(endpoint(&endpoint_id, Confidence::Exact, &exact_id));

        let map = builder.build();
        let contract = map.endpoints[0].contract.as_ref().expect("merged contract");
        assert_eq!(contract.confidence, Confidence::Exact);
        assert_eq!(contract.request.parameters[0].schema_id, exact_id);
        assert_eq!(map.schemas.len(), 1);
        assert_eq!(map.schemas[0].id, exact_id);
        assert!(!map.schemas.iter().any(|schema| schema.id == orphan_id));
    }

    #[test]
    fn truncates_nested_input_before_merging_the_model() {
        let endpoint_id = canonical_endpoint_id("GET", "/users/{id}");
        let mut endpoint = endpoint(&endpoint_id, Confidence::Exact, "schema:string");
        endpoint.tags = (0..=super::MAX_NESTED_ITEMS)
            .map(|index| format!("tag-{index:04}"))
            .collect();
        let mut builder = ApiMapBuilder::new("bounded");
        builder.add_endpoint(endpoint);

        let map = builder.build();
        assert_eq!(map.endpoints[0].tags.len(), super::MAX_NESTED_ITEMS);
        assert!(
            map.diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "model-budget-truncated")
        );
    }

    fn endpoint(id: &str, confidence: Confidence, parameter_schema: &str) -> Endpoint {
        Endpoint {
            id: id.to_owned(),
            method: "GET".to_owned(),
            path: "/users/{id}".to_owned(),
            display_path: "/users/{id}".to_owned(),
            district: district_for_path("/users/{id}"),
            framework: "test".to_owned(),
            operation_id: None,
            tags: Vec::new(),
            sources: Vec::new(),
            spec_only: false,
            contract: Some(EndpointContract {
                confidence,
                request: RequestContract {
                    parameters: vec![ParameterContract {
                        name: "id".to_owned(),
                        location: ParameterLocation::Path,
                        required: true,
                        schema_id: parameter_schema.to_owned(),
                    }],
                    bodies: Vec::new(),
                },
                responses: Vec::new(),
                evidence: Vec::new(),
            }),
        }
    }
}
