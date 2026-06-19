export type DependencyKind = "middleware" | "service" | "datastore" | "external";
export type Confidence = "exact" | "inferred";
export type ParameterLocation = "path" | "query" | "header" | "cookie";
export type SchemaKind =
  | "object"
  | "array"
  | "string"
  | "integer"
  | "number"
  | "boolean"
  | "null"
  | "union"
  | "intersection"
  | "unknown";

export interface SourceRef {
  file: string;
  line: number;
  column: number;
}

export interface Evidence {
  kind: string;
  detail: string;
  source?: SourceRef;
}

export interface SchemaLiteral {
  kind: "string" | "integer" | "number" | "boolean" | "null";
  value: string;
}

export interface SchemaProperty {
  name: string;
  schema_id: string;
  required: boolean;
}

export interface SchemaConstraints {
  min_length?: number;
  max_length?: number;
  minimum?: string;
  maximum?: string;
  min_items?: number;
  max_items?: number;
  pattern?: string;
}

export interface ApiSchema {
  id: string;
  name?: string;
  kind: SchemaKind;
  format?: string;
  properties?: SchemaProperty[];
  items?: string;
  variants?: string[];
  enum_values?: SchemaLiteral[];
  const_value?: SchemaLiteral;
  constraints?: SchemaConstraints;
  nullable?: boolean;
  confidence: Confidence;
  evidence?: Evidence[];
}

export interface ParameterContract {
  name: string;
  location: ParameterLocation;
  required: boolean;
  schema_id: string;
}

export interface ContentContract {
  media_type: string;
  schema_id: string;
  required?: boolean;
}

export interface ResponseContract {
  status: string;
  contents?: ContentContract[];
}

export interface EndpointContract {
  confidence: Confidence;
  request?: {
    parameters?: ParameterContract[];
    bodies?: ContentContract[];
  };
  responses?: ResponseContract[];
  evidence?: Evidence[];
}

export interface Endpoint {
  id: string;
  method: string;
  path: string;
  display_path: string;
  framework: string;
  operation_id?: string;
  tags?: string[];
  sources?: SourceRef[];
  spec_only?: boolean;
  contract?: EndpointContract;
}

export interface Dependency {
  id: string;
  name: string;
  kind: DependencyKind;
  pinned?: boolean;
}

export interface Relation {
  endpoint_id: string;
  dependency_id: string;
  confidence: Confidence;
  evidence: Evidence[];
}

export interface ApiMap {
  endpoints: Endpoint[];
  schemas?: ApiSchema[];
  dependencies: Dependency[];
  relations: Relation[];
  diagnostics: Array<{
    code: string;
    severity: string;
    message: string;
    source?: SourceRef;
  }>;
}
