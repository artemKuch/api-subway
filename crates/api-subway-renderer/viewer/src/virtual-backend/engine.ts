import {
  createSchemaIndex,
  generateValue,
  seededRandom,
  validateValue,
  type JsonValue,
} from "../schema-simulator";
import type {
  ApiMap,
  ApiSchema,
  Endpoint,
  ParameterLocation,
} from "../types";
import { projectValueToSchema } from "./schema-projector";
import { VirtualBackendStore } from "./store";
import { createDictionary, setDictionaryValue } from "./dictionary";
import {
  isJsonObject,
  type ExecutionResult,
  type JsonObject,
  type VirtualOperation,
  type VirtualRequest,
} from "./types";

const parameterLocations: ParameterLocation[] = [
  "path",
  "query",
  "header",
  "cookie",
];

export class VirtualBackendEngine {
  private readonly endpoints: Map<string, Endpoint>;
  private readonly schemas: Map<string, ApiSchema>;

  constructor(
    private readonly map: ApiMap,
    private readonly store: VirtualBackendStore,
  ) {
    this.endpoints = new Map(
      map.endpoints.map((endpoint) => [endpoint.id, endpoint]),
    );
    this.schemas = createSchemaIndex(map.schemas);
  }

  defaultRequest(endpointId: string): VirtualRequest {
    const endpoint = this.requireEndpoint(endpointId);
    const operation = this.requireOperation(endpointId);
    const parameters = emptyParameters();
    const resource = this.store.resource(operation.resource);
    for (const parameter of endpoint.contract?.request?.parameters ?? []) {
      const recordValue =
        parameter.location === "path" &&
        parameter.name === operation.pathParameter
          ? resource?.records[0]?.[resource.primaryKey]
          : undefined;
      setDictionaryValue(
        parameters[parameter.location],
        parameter.name,
        recordValue ??
          generateValue(
            parameter.schema_id,
            this.schemas,
            seededRandom(endpointSeed(endpointId, parameter.name)),
          ),
      );
    }
    const bodySchema = endpoint.contract?.request?.bodies?.[0]?.schema_id;
    const body = bodySchema
      ? generateValue(
          bodySchema,
          this.schemas,
          seededRandom(endpointSeed(endpointId, "body")),
        )
      : null;
    return { parameters, body };
  }

  execute(endpointId: string, request: VirtualRequest): ExecutionResult {
    const endpoint = this.requireEndpoint(endpointId);
    const operation = this.requireOperation(endpointId);
    const requestErrors = this.validateRequest(endpoint, request);
    if (requestErrors.length > 0) {
      const status = this.errorStatus(endpoint, ["422", "400"]);
      return this.finalizeResult(
        endpoint,
        {
          endpointId,
          status,
          body: {
            error: "Request validation failed",
            message: "Request validation failed",
            details: requestErrors,
          },
        },
        requestErrors,
      );
    }

    const result = this.executeOperation(endpoint, operation, request);
    return this.finalizeResult(endpoint, result, requestErrors);
  }

  operation(endpointId: string): VirtualOperation {
    return this.requireOperation(endpointId);
  }

  private executeOperation(
    endpoint: Endpoint,
    operation: VirtualOperation,
    request: VirtualRequest,
  ): Omit<ExecutionResult, "requestErrors" | "responseErrors"> {
    const resource = this.store.resource(operation.resource);
    if (!resource) {
      return this.errorResult(
        endpoint,
        ["500"],
        "Virtual resource is unavailable",
      );
    }
    const records = resource.records;
    const primaryKey = resource.primaryKey;
    const requestedId = operation.pathParameter
      ? request.parameters.path[operation.pathParameter]
      : undefined;
    const recordIndex = records.findIndex((record) =>
      sameValue(record[primaryKey], requestedId),
    );

    switch (operation.kind) {
      case "list": {
        const filtered = filterRecords(records, request.parameters.query);
        return this.successResult(
          endpoint,
          operation,
          this.listResponse(operation, filtered),
        );
      }
      case "read":
        return recordIndex >= 0
          ? this.successResult(endpoint, operation, records[recordIndex]!)
          : this.errorResult(endpoint, ["404"], "Record not found");
      case "create": {
        if (!isJsonObject(request.body)) {
          return this.errorResult(
            endpoint,
            ["422", "400"],
            "Request body must be an object",
          );
        }
        const nextRecord: JsonObject = { ...request.body };
        if (!Object.hasOwn(nextRecord, primaryKey)) {
          setDictionaryValue(
            nextRecord,
            primaryKey,
            nextIdentifier(operation.resource, primaryKey, records),
          );
        }
        this.store.commitResource(
          operation.resource,
          [...records, nextRecord],
          endpoint.id,
        );
        return this.successResult(
          endpoint,
          operation,
          nextRecord,
          operation.resource,
        );
      }
      case "replace":
      case "update": {
        if (!isJsonObject(request.body)) {
          return this.errorResult(
            endpoint,
            ["422", "400"],
            "Request body must be an object",
          );
        }
        if (recordIndex < 0) {
          return this.errorResult(endpoint, ["404"], "Record not found");
        }
        const current = records[recordIndex]!;
        const nextRecord: JsonObject =
          operation.kind === "update"
            ? { ...current, ...request.body }
            : { ...request.body };
        setDictionaryValue(
          nextRecord,
          primaryKey,
          current[primaryKey] ?? requestedId ?? null,
        );
        const nextRecords = [...records];
        nextRecords[recordIndex] = nextRecord;
        this.store.commitResource(operation.resource, nextRecords, endpoint.id);
        return this.successResult(
          endpoint,
          operation,
          nextRecord,
          operation.resource,
        );
      }
      case "delete": {
        if (recordIndex < 0) {
          return this.errorResult(endpoint, ["404"], "Record not found");
        }
        this.store.commitResource(
          operation.resource,
          records.filter((_, index) => index !== recordIndex),
          endpoint.id,
        );
        return this.successResult(
          endpoint,
          operation,
          null,
          operation.resource,
        );
      }
      case "head":
        return this.successResult(endpoint, operation, null);
      case "options": {
        const operations = this.store.operations();
        const methods = this.map.endpoints
          .filter(
            (candidate) =>
              operations[candidate.id]?.resource === operation.resource,
          )
          .map((candidate) => candidate.method);
        return this.successResult(endpoint, operation, {
          allow: [...new Set(methods)],
        });
      }
      case "action":
        return this.successResult(endpoint, operation, request.body);
    }
  }

  private validateRequest(endpoint: Endpoint, request: VirtualRequest): string[] {
    const errors: string[] = [];
    for (const parameter of endpoint.contract?.request?.parameters ?? []) {
      const value = request.parameters[parameter.location][parameter.name];
      if (value === undefined) {
        if (parameter.required) {
          errors.push(`${parameter.location}.${parameter.name} is required`);
        }
        continue;
      }
      errors.push(
        ...validateValue(
          parameter.schema_id,
          value,
          this.schemas,
          `${parameter.location}.${parameter.name}`,
        ),
      );
    }
    const body = endpoint.contract?.request?.bodies?.[0];
    if (body) {
      if (request.body === null && body.required) {
        errors.push("body is required");
      } else if (request.body !== null) {
        errors.push(
          ...validateValue(body.schema_id, request.body, this.schemas, "body"),
        );
      }
    }
    return errors;
  }

  private validateResponse(
    schemaId: string | undefined,
    body: JsonValue,
  ): string[] {
    if (!schemaId) return [];
    return validateValue(schemaId, body, this.schemas, "response");
  }

  private listResponse(
    operation: VirtualOperation,
    records: JsonObject[],
  ): JsonValue {
    if (!operation.responseSchemaId) return records;
    const schema = operation.responseSchemaId
      ? this.schemas.get(operation.responseSchemaId)
      : undefined;
    if (schema?.kind === "array") return records;
    const collectionProperty = schema?.properties?.find((property) => {
      if (!/^(?:data|items|records|results)$/i.test(property.name)) {
        return false;
      }
      const propertySchema = this.schemas.get(property.schema_id);
      return propertySchema?.kind === "array";
    });
    if (collectionProperty) {
      return {
        [collectionProperty.name]: records,
        total: records.length,
      };
    }
    return { items: records, total: records.length };
  }

  private finalizeResult(
    endpoint: Endpoint,
    result: Omit<ExecutionResult, "requestErrors" | "responseErrors">,
    requestErrors: string[],
  ): ExecutionResult {
    const schemaId = this.responseSchemaId(endpoint, result.status);
    const body = schemaId
      ? projectValueToSchema(
          schemaId,
          result.body,
          this.schemas,
          seededRandom(
            endpointSeed(endpoint.id, `response:${result.status}`),
          ),
        )
      : result.body;
    return {
      ...result,
      body,
      requestErrors,
      responseErrors: this.validateResponse(schemaId, body),
    };
  }

  private responseSchemaId(
    endpoint: Endpoint,
    status: string,
  ): string | undefined {
    const responses = endpoint.contract?.responses ?? [];
    return (
      responses.find((response) => response.status === status) ??
      responses.find(
        (response) =>
          /^[1-5]XX$/i.test(response.status) &&
          response.status[0] === status[0],
      ) ??
      responses.find((response) => response.status === "default")
    )?.contents?.[0]?.schema_id;
  }

  private successResult(
    endpoint: Endpoint,
    operation: VirtualOperation,
    body: JsonValue,
    changedResource?: string,
  ): Omit<ExecutionResult, "requestErrors" | "responseErrors"> {
    return {
      endpointId: endpoint.id,
      status: operation.responseStatus,
      body,
      changedResource,
    };
  }

  private errorResult(
    endpoint: Endpoint,
    preferredStatuses: string[],
    message: string,
  ): Omit<ExecutionResult, "requestErrors" | "responseErrors"> {
    const status = this.errorStatus(endpoint, preferredStatuses);
    return {
      endpointId: endpoint.id,
      status,
      body: this.responseSchemaId(endpoint, status)
        ? { error: message, message }
        : { error: message },
    };
  }

  private errorStatus(endpoint: Endpoint, preferred: string[]): string {
    const declared = endpoint.contract?.responses ?? [];
    return (
      preferred.find((status) =>
        declared.some((response) => response.status === status),
      ) ?? preferred[0] ?? "500"
    );
  }

  private requireEndpoint(endpointId: string): Endpoint {
    const endpoint = this.endpoints.get(endpointId);
    if (!endpoint) throw new Error(`Unknown endpoint: ${endpointId}`);
    return endpoint;
  }

  private requireOperation(endpointId: string): VirtualOperation {
    const operation = this.store.operation(endpointId);
    if (!operation) throw new Error(`No virtual operation for ${endpointId}`);
    return operation;
  }
}

const emptyParameters = (): VirtualRequest["parameters"] => ({
  path: createDictionary<JsonValue>(),
  query: createDictionary<JsonValue>(),
  header: createDictionary<JsonValue>(),
  cookie: createDictionary<JsonValue>(),
});

const filterRecords = (
  records: JsonObject[],
  query: Record<string, JsonValue>,
): JsonObject[] => {
  const ignored = new Set(["page", "limit", "offset", "cursor"]);
  const filters = Object.entries(query).filter(
    ([key, value]) => !ignored.has(key) && value !== null && value !== "",
  );
  const filtered = records.filter((record) =>
    filters.every(([key, value]) => sameValue(record[key], value)),
  );
  const limit = Math.min(100, numericQuery(query.limit) ?? filtered.length);
  const page = numericQuery(query.page);
  const offset =
    numericQuery(query.offset) ??
    (page !== undefined && page > 0 ? (page - 1) * limit : 0);
  return filtered.slice(offset, offset + limit);
};

const numericQuery = (value: JsonValue | undefined): number | undefined => {
  const parsed = typeof value === "number" ? value : Number(value);
  return Number.isFinite(parsed) && parsed >= 0 ? Math.floor(parsed) : undefined;
};

const endpointSeed = (endpointId: string, field: string): number =>
  recordSeed({ endpointId, field });

const recordSeed = (record: Record<string, JsonValue>): number => {
  const serialized = JSON.stringify(record);
  let hash = 2_166_136_261;
  for (const character of serialized) {
    hash ^= character.codePointAt(0) ?? 0;
    hash = Math.imul(hash, 16_777_619);
  }
  return (hash >>> 0) || 1;
};

const sameValue = (
  left: JsonValue | undefined,
  right: JsonValue | undefined,
): boolean => {
  if (
    (typeof left === "string" || typeof left === "number") &&
    (typeof right === "string" || typeof right === "number")
  ) {
    return String(left) === String(right);
  }
  return JSON.stringify(left) === JSON.stringify(right);
};

const nextIdentifier = (
  resource: string,
  primaryKey: string,
  records: JsonObject[],
): string => {
  const prefix = resource.endsWith("s") ? resource.slice(0, -1) : resource;
  const usedIdentifiers = new Set(
    records.map((record) => String(record[primaryKey])),
  );
  for (let sequence = 1; sequence <= records.length + 1; sequence += 1) {
    const candidate = `${prefix}-${String(sequence).padStart(3, "0")}`;
    if (!usedIdentifiers.has(candidate)) return candidate;
  }
  throw new Error(`Could not allocate an identifier for ${resource}`);
};
