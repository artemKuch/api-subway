import type { JsonValue } from "../schema-simulator";
import type { Confidence, ParameterLocation } from "../types";

export type JsonObject = { [key: string]: JsonValue };

export type VirtualOperationKind =
  | "list"
  | "read"
  | "create"
  | "replace"
  | "update"
  | "delete"
  | "head"
  | "options"
  | "action";

export interface VirtualResource {
  primaryKey: string;
  records: JsonObject[];
}

export interface VirtualOperation {
  endpointId: string;
  kind: VirtualOperationKind;
  resource: string;
  primaryKey: string;
  pathParameter?: string;
  responseStatus: string;
  responseSchemaId?: string;
  confidence: Confidence;
}

export interface VirtualBackendSnapshot {
  resources: Record<string, VirtualResource>;
  operations: Record<string, VirtualOperation>;
}

export interface EditableBackendSnapshot {
  resources: Record<string, VirtualResource>;
}

export interface VirtualRequest {
  parameters: Record<ParameterLocation, Record<string, JsonValue>>;
  body: JsonValue;
}

export interface ExecutionResult {
  endpointId: string;
  status: string;
  body: JsonValue;
  requestErrors: string[];
  responseErrors: string[];
  changedResource?: string;
}

export interface BackendChange {
  endpointId: string;
  resource: string;
}

export const isJsonObject = (value: JsonValue): value is JsonObject =>
  typeof value === "object" && value !== null && !Array.isArray(value);
