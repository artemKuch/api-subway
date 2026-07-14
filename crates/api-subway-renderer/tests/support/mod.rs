use api_subway_core::{
    ApiMapBuilder, ApiMapV1, ApiSchema, Confidence, ContentContract, Dependency, DependencyKind,
    Endpoint, EndpointContract, Evidence, EvidenceKind, LiteralKind, ParameterContract,
    ParameterLocation, Relation, RequestContract, ResponseContract, SchemaConstraints, SchemaKind,
    SchemaLiteral, SchemaProperty, SourceRef, canonical_endpoint_id, dependency_id,
    district_for_path, schema_id,
};

pub fn synthetic_map(station_count: usize) -> ApiMapV1 {
    let mut builder = ApiMapBuilder::new(format!("Synthetic {station_count}"));
    builder.add_framework("synthetic");
    let lines = [
        ("Auth", DependencyKind::Middleware),
        ("Rate limit", DependencyKind::Middleware),
        ("Prisma", DependencyKind::Datastore),
        ("Redis", DependencyKind::Datastore),
        ("Stripe", DependencyKind::External),
        ("OpenAI", DependencyKind::External),
        ("Notifications", DependencyKind::Service),
        ("Audit", DependencyKind::Service),
    ];
    for (name, kind) in lines {
        builder.add_dependency(Dependency {
            id: dependency_id(kind, name),
            name: name.to_owned(),
            kind,
            pinned: name == "Auth",
            packages: Vec::new(),
        });
    }
    add_contract_schemas(&mut builder);

    let methods = ["GET", "POST", "PUT", "PATCH", "DELETE"];
    let districts = ["users", "orders", "admin", "reports", "billing", "events"];
    for index in 0..station_count {
        let district = districts[index % districts.len()];
        let (method, path) = match index {
            1 => ("GET", "/orders".to_owned()),
            7 => ("PUT", "/orders/{id}".to_owned()),
            13 => ("POST", "/orders".to_owned()),
            19 => ("GET", "/orders/{id}".to_owned()),
            _ => {
                let method = methods[index % methods.len()];
                let path = if index % 3 == 0 {
                    format!("/{district}/item-{index:02}/{{id}}")
                } else {
                    format!("/{district}/action-{index:02}")
                };
                (method, path)
            }
        };
        let endpoint_id = canonical_endpoint_id(method, &path);
        let source = SourceRef {
            file: format!("src/routes/{district}.ts"),
            line: u32::try_from(index + 1).unwrap_or(u32::MAX),
            column: 1,
        };
        let mut parameters = Vec::new();
        if path.contains("{id}") {
            parameters.push(ParameterContract {
                name: "id".to_owned(),
                location: ParameterLocation::Path,
                required: true,
                schema_id: schema_id("synthetic:uuid"),
            });
        }
        let bodies = matches!(method, "POST" | "PUT" | "PATCH")
            .then(|| ContentContract {
                media_type: "application/json".to_owned(),
                schema_id: schema_id("synthetic:input"),
                required: true,
            })
            .into_iter()
            .collect();
        let success = if method == "POST" { "201" } else { "200" };
        builder.add_endpoint(Endpoint {
            id: endpoint_id.clone(),
            method: method.to_owned(),
            path: path.clone(),
            display_path: path.clone(),
            district: district_for_path(&path),
            framework: "synthetic".to_owned(),
            operation_id: Some(format!("operation{index}")),
            tags: vec![district.to_owned()],
            sources: vec![source.clone()],
            spec_only: false,
            contract: Some(EndpointContract {
                confidence: Confidence::Exact,
                request: RequestContract { parameters, bodies },
                responses: vec![
                    ResponseContract {
                        status: success.to_owned(),
                        contents: vec![ContentContract {
                            media_type: "application/json".to_owned(),
                            schema_id: schema_id(if index == 1 {
                                "synthetic:outputs"
                            } else {
                                "synthetic:output"
                            }),
                            required: false,
                        }],
                    },
                    ResponseContract {
                        status: "400".to_owned(),
                        contents: vec![ContentContract {
                            media_type: "application/json".to_owned(),
                            schema_id: schema_id("synthetic:error"),
                            required: false,
                        }],
                    },
                ],
                evidence: vec![Evidence {
                    kind: EvidenceKind::OpenApi,
                    detail: "Synthetic OpenAPI request and response contract".to_owned(),
                    source: Some(source),
                }],
            }),
        });
        for (line_index, (name, kind)) in lines.iter().enumerate() {
            if (index + line_index) % (line_index % 3 + 2) != 0 {
                continue;
            }
            builder.add_relation(Relation {
                endpoint_id: endpoint_id.clone(),
                dependency_id: dependency_id(*kind, name),
                confidence: if (index + line_index) % 5 == 0 {
                    Confidence::Inferred
                } else {
                    Confidence::Exact
                },
                evidence: vec![Evidence {
                    kind: EvidenceKind::Call,
                    detail: format!("Synthetic evidence for {name}"),
                    source: Some(SourceRef {
                        file: format!("src/routes/{district}.ts"),
                        line: u32::try_from(index + 1).unwrap_or(u32::MAX),
                        column: 1,
                    }),
                }],
            });
        }
    }
    builder.build()
}

fn add_contract_schemas(builder: &mut ApiMapBuilder) {
    for schema in [
        primitive_schema("synthetic:string", "Text", SchemaKind::String, None),
        primitive_schema(
            "synthetic:email",
            "Email",
            SchemaKind::String,
            Some("email"),
        ),
        primitive_schema(
            "synthetic:uuid",
            "Identifier",
            SchemaKind::String,
            Some("uuid"),
        ),
        ApiSchema {
            id: schema_id("synthetic:role"),
            name: Some("Role".to_owned()),
            kind: SchemaKind::String,
            format: None,
            properties: Vec::new(),
            items: None,
            variants: Vec::new(),
            enum_values: ["admin", "member"]
                .into_iter()
                .map(|value| SchemaLiteral {
                    kind: LiteralKind::String,
                    value: value.to_owned(),
                })
                .collect(),
            const_value: None,
            constraints: SchemaConstraints::default(),
            nullable: false,
            confidence: Confidence::Exact,
            evidence: contract_schema_evidence(),
        },
        object_schema(
            "synthetic:input",
            "Request payload",
            [
                ("name", "synthetic:string", true),
                ("email", "synthetic:email", true),
                ("role", "synthetic:role", true),
            ],
        ),
        object_schema(
            "synthetic:output",
            "Response payload",
            [
                ("id", "synthetic:uuid", true),
                ("name", "synthetic:string", true),
                ("email", "synthetic:email", true),
            ],
        ),
        ApiSchema {
            id: schema_id("synthetic:outputs"),
            name: Some("Response list".to_owned()),
            kind: SchemaKind::Array,
            format: None,
            properties: Vec::new(),
            items: Some(schema_id("synthetic:output")),
            variants: Vec::new(),
            enum_values: Vec::new(),
            const_value: None,
            constraints: SchemaConstraints::default(),
            nullable: false,
            confidence: Confidence::Exact,
            evidence: contract_schema_evidence(),
        },
        object_schema(
            "synthetic:error",
            "Error",
            [("message", "synthetic:string", true)],
        ),
    ] {
        builder.add_schema(schema);
    }
}

fn primitive_schema(origin: &str, name: &str, kind: SchemaKind, format: Option<&str>) -> ApiSchema {
    ApiSchema {
        id: schema_id(origin),
        name: Some(name.to_owned()),
        kind,
        format: format.map(str::to_owned),
        properties: Vec::new(),
        items: None,
        variants: Vec::new(),
        enum_values: Vec::new(),
        const_value: None,
        constraints: SchemaConstraints::default(),
        nullable: false,
        confidence: Confidence::Exact,
        evidence: contract_schema_evidence(),
    }
}

fn object_schema<const N: usize>(
    origin: &str,
    name: &str,
    properties: [(&str, &str, bool); N],
) -> ApiSchema {
    ApiSchema {
        id: schema_id(origin),
        name: Some(name.to_owned()),
        kind: SchemaKind::Object,
        format: None,
        properties: properties
            .into_iter()
            .map(|(name, schema, required)| SchemaProperty {
                name: name.to_owned(),
                schema_id: schema_id(schema),
                required,
            })
            .collect(),
        items: None,
        variants: Vec::new(),
        enum_values: Vec::new(),
        const_value: None,
        constraints: SchemaConstraints::default(),
        nullable: false,
        confidence: Confidence::Exact,
        evidence: contract_schema_evidence(),
    }
}

fn contract_schema_evidence() -> Vec<Evidence> {
    vec![Evidence {
        kind: EvidenceKind::OpenApi,
        detail: "Synthetic contract schema".to_owned(),
        source: Some(SourceRef {
            file: "openapi.yaml".to_owned(),
            line: 1,
            column: 1,
        }),
    }]
}
