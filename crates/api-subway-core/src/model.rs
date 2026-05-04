use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiMapV1 {
    pub schema_version: u32,
    pub project: ProjectInfo,
    pub endpoints: Vec<Endpoint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schemas: Vec<ApiSchema>,
    pub dependencies: Vec<Dependency>,
    pub relations: Vec<Relation>,
    pub diagnostics: Vec<Diagnostic>,
}

impl ApiMapV1 {
    pub const SCHEMA_VERSION: u32 = 1;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProjectInfo {
    pub name: String,
    pub root: String,
    pub frameworks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Endpoint {
    pub id: String,
    pub method: String,
    pub path: String,
    pub display_path: String,
    pub district: String,
    pub framework: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<SourceRef>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub spec_only: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contract: Option<EndpointContract>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct EndpointContract {
    pub confidence: Confidence,
    #[serde(default, skip_serializing_if = "RequestContract::is_empty")]
    pub request: RequestContract,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub responses: Vec<ResponseContract>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct RequestContract {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<ParameterContract>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bodies: Vec<ContentContract>,
}

impl RequestContract {
    pub fn is_empty(&self) -> bool {
        self.parameters.is_empty() && self.bodies.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ParameterContract {
    pub name: String,
    pub location: ParameterLocation,
    pub required: bool,
    pub schema_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum ParameterLocation {
    Path,
    Query,
    Header,
    Cookie,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ContentContract {
    pub media_type: String,
    pub schema_id: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResponseContract {
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contents: Vec<ContentContract>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ApiSchema {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub kind: SchemaKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<SchemaProperty>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enum_values: Vec<SchemaLiteral>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub const_value: Option<SchemaLiteral>,
    #[serde(default, skip_serializing_if = "SchemaConstraints::is_empty")]
    pub constraints: SchemaConstraints,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub nullable: bool,
    pub confidence: Confidence,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum SchemaKind {
    Object,
    Array,
    String,
    Integer,
    Number,
    Boolean,
    Null,
    Union,
    Intersection,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct SchemaProperty {
    pub name: String,
    pub schema_id: String,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct SchemaLiteral {
    pub kind: LiteralKind,
    pub value: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum LiteralKind {
    String,
    Integer,
    Number,
    Boolean,
    Null,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct SchemaConstraints {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_length: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_items: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
}

impl SchemaConstraints {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Dependency {
    pub id: String,
    pub name: String,
    pub kind: DependencyKind,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum DependencyKind {
    Middleware,
    Service,
    Datastore,
    External,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Relation {
    pub endpoint_id: String,
    pub dependency_id: String,
    pub confidence: Confidence,
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Inferred,
    Exact,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Evidence {
    pub kind: EvidenceKind,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceRef>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceKind {
    Framework,
    Call,
    Import,
    Configuration,
    OpenApi,
    Heuristic,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct SourceRef {
    pub file: String,
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Diagnostic {
    pub code: String,
    pub severity: DiagnosticSeverity,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceRef>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}
