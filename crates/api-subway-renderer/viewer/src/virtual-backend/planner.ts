import {
  createSchemaIndex,
  generateValue,
  seededRandom,
} from "../schema-simulator";
import type {
  ApiMap,
  ApiSchema,
  Endpoint,
  ResponseContract,
} from "../types";
import {
  isJsonObject,
  type JsonObject,
  type VirtualBackendSnapshot,
  type VirtualOperation,
  type VirtualOperationKind,
  type VirtualResource,
} from "./types";
import { createDictionary, setDictionaryValue } from "./dictionary";

const MAX_PLANNED_RESOURCES = 200;
const OVERFLOW_RESOURCE = "api-subway-overflow";
const MAX_RESOURCE_NAME_LENGTH = 128;

interface PathShape {
  resource: string;
  resourceKey: string;
  pathParameter?: string;
  itemRoute: boolean;
  actionRoute: boolean;
}

export const planVirtualBackend = (
  map: ApiMap,
  seed = 42,
): VirtualBackendSnapshot => {
  const schemas = createSchemaIndex(map.schemas);
  const operations = createDictionary<VirtualOperation>();
  const resourceEndpoints = new Map<string, Endpoint[]>();
  const resourceNames = new Map<string, string>();
  const usedResourceNames = new Set<string>();

  for (const endpoint of map.endpoints) {
    const shape = describePath(endpoint.path);
    const knownResource = resourceNames.get(shape.resourceKey);
    const overflow = !knownResource && resourceNames.size >= MAX_PLANNED_RESOURCES - 1;
    const resource =
      knownResource ??
      (overflow
        ? OVERFLOW_RESOURCE
        : allocateResourceName(
            shape.resource,
            shape.resourceKey,
            usedResourceNames,
          ));
    resourceNames.set(shape.resourceKey, resource);
    usedResourceNames.add(resource);
    const response = selectPrimaryResponse(endpoint);
    const operation: VirtualOperation = {
      endpointId: endpoint.id,
      kind: operationKind(endpoint.method, shape),
      resource,
      primaryKey: primaryKeyFor(shape.pathParameter),
      pathParameter: shape.pathParameter,
      responseStatus: concreteStatus(endpoint.method, response?.status),
      responseSchemaId: response?.contents?.[0]?.schema_id,
      confidence: shape.actionRoute || overflow ? "inferred" : "exact",
    };
    setDictionaryValue(operations, endpoint.id, operation);
    const endpoints = resourceEndpoints.get(resource) ?? [];
    endpoints.push(endpoint);
    resourceEndpoints.set(resource, endpoints);
  }

  const resources = createDictionary<VirtualResource>();
  for (const [resource, endpoints] of resourceEndpoints) {
    const operation = endpoints
      .map((endpoint) => operations[endpoint.id])
      .find((candidate) => candidate?.pathParameter);
    const primaryKey = operation?.primaryKey ?? "id";
    setDictionaryValue(resources, resource, {
      primaryKey,
      records: seedRecords(resource, primaryKey, endpoints, schemas, seed),
    });
  }

  return { resources, operations };
};

const describePath = (path: string): PathShape => {
  const segments = path.split("/").filter(Boolean);
  if (segments.length === 0) {
    return {
      resource: "root",
      resourceKey: "root",
      itemRoute: false,
      actionRoute: false,
    };
  }
  const last = segments.at(-1) ?? "root";
  const lastParameter = parameterName(last);
  const pathParameter = segments
    .map(parameterName)
    .filter((parameter): parameter is string => Boolean(parameter))
    .at(-1);
  const recognizedAction = !lastParameter && isActionSegment(last);
  const actionRoute = recognizedAction;
  const staticSegments = segments.filter(
    (segment) => !parameterName(segment) && !isInfrastructureSegment(segment),
  );
  const resourceSegments =
    actionRoute && staticSegments.length > 1
      ? staticSegments.slice(0, -1)
      : staticSegments;
  const resourceKey = (resourceSegments.length > 0
    ? resourceSegments
    : [last]
  ).join("/");
  return {
    resource: sanitizeResourceName(resourceSegments.at(-1) ?? last ?? "root"),
    resourceKey,
    pathParameter,
    itemRoute: Boolean(lastParameter),
    actionRoute,
  };
};

const isInfrastructureSegment = (segment: string): boolean =>
  /^(?:api|rest|graphql|v\d+(?:\.\d+)?)$/i.test(segment);

const isActionSegment = (segment: string): boolean =>
  /^(?:action|search|login|logout|callback|health|status|sync|export|import|verify|cancel|confirm|preview|send|retry|approve|reject)(?:[-_].*)?$/i.test(
    segment,
  );

const operationKind = (
  method: string,
  shape: PathShape,
): VirtualOperationKind => {
  if (shape.actionRoute) return "action";
  switch (method) {
    case "GET":
      return shape.itemRoute ? "read" : "list";
    case "POST":
      return shape.itemRoute ? "action" : "create";
    case "PUT":
      return shape.itemRoute ? "replace" : "action";
    case "PATCH":
      return shape.itemRoute ? "update" : "action";
    case "DELETE":
      return shape.itemRoute ? "delete" : "action";
    case "HEAD":
      return "head";
    case "OPTIONS":
      return "options";
    default:
      return "action";
  }
};

const seedRecords = (
  resource: string,
  primaryKey: string,
  endpoints: Endpoint[],
  schemas: Map<string, ApiSchema>,
  seed: number,
): JsonObject[] => {
  const schemaId = entitySchemaId(endpoints, schemas);
  return [0, 1].map((offset) => {
    const generated = schemaId
      ? generateValue(schemaId, schemas, seededRandom(seed + offset + resource.length))
      : {};
    const record: JsonObject = isJsonObject(generated)
      ? { ...generated }
      : { value: generated };
    if (!Object.hasOwn(record, primaryKey)) {
      record[primaryKey] = `${singular(resource)}-${String(offset + 1).padStart(3, "0")}`;
    }
    return record;
  });
};

const entitySchemaId = (
  endpoints: Endpoint[],
  schemas: Map<string, ApiSchema>,
): string | undefined => {
  for (const endpoint of endpoints) {
    const operationResponse = selectPrimaryResponse(endpoint)?.contents?.[0]
      ?.schema_id;
    if (!operationResponse) continue;
    const schema = schemas.get(operationResponse);
    if (schema?.kind === "array" && schema.items) return schema.items;
    if (schema?.kind === "object") {
      const collectionProperty = schema.properties?.find((property) => {
        if (!/^(?:data|items|records|results)$/i.test(property.name)) {
          return false;
        }
        const propertySchema = schemas.get(property.schema_id);
        return propertySchema?.kind === "array" && propertySchema.items;
      });
      const collectionSchema = collectionProperty
        ? schemas.get(collectionProperty.schema_id)
        : undefined;
      return collectionSchema?.items ?? operationResponse;
    }
  }
  return endpoints
    .flatMap((endpoint) => endpoint.contract?.request?.bodies ?? [])
    .map((content) => content.schema_id)
    .find((schemaId) => schemas.get(schemaId)?.kind === "object");
};

const parameterName = (segment: string): string | undefined => {
  const match = /^\{(?:\*{0,2})?([^}]+)\}$/.exec(segment);
  return match?.[1];
};

const primaryKeyFor = (pathParameter: string | undefined): string =>
  pathParameter && !/id$/i.test(pathParameter) ? pathParameter : "id";

const sanitizeResourceName = (value: string): string => {
  const normalized = value
    .replace(/^\{+|\}+$/g, "")
    .replace(/[^a-zA-Z0-9_-]/g, "-")
    .toLocaleLowerCase();
  return (normalized || "root").slice(0, MAX_RESOURCE_NAME_LENGTH);
};

const allocateResourceName = (
  base: string,
  identity: string,
  used: Set<string>,
): string => {
  if (!used.has(base)) return base;
  const hash = stableHash(identity);
  for (let attempt = 0; attempt <= used.size; attempt += 1) {
    const suffix = attempt === 0 ? hash : `${hash}-${attempt}`;
    const candidate = `${base.slice(0, MAX_RESOURCE_NAME_LENGTH - suffix.length - 1)}-${suffix}`;
    if (!used.has(candidate)) return candidate;
  }
  throw new Error("Could not allocate a unique virtual resource name");
};

const stableHash = (value: string): string => {
  let hash = 2_166_136_261;
  for (const character of value) {
    hash ^= character.codePointAt(0) ?? 0;
    hash = Math.imul(hash, 16_777_619);
  }
  return (hash >>> 0).toString(16).padStart(8, "0");
};

const singular = (resource: string): string =>
  resource.endsWith("ies")
    ? `${resource.slice(0, -3)}y`
    : resource.endsWith("s")
      ? resource.slice(0, -1)
      : resource;

const defaultStatus = (method: string): string => {
  if (method === "POST") return "201";
  if (method === "DELETE") return "204";
  return "200";
};

const concreteStatus = (method: string, declared: string | undefined): string => {
  if (declared && /^[1-5]\d\d$/.test(declared)) return declared;
  if (declared && /^[1-5]XX$/i.test(declared)) {
    const methodDefault = defaultStatus(method);
    return methodDefault[0] === declared[0] ? methodDefault : `${declared[0]}00`;
  }
  return defaultStatus(method);
};

const selectPrimaryResponse = (
  endpoint: Endpoint,
): ResponseContract | undefined => {
  const responses = endpoint.contract?.responses ?? [];
  return (
    responses.find(isSuccessfulResponseWithContent) ??
    responses.find(isSuccessfulResponse) ??
    responses.find(hasResponseContent) ??
    responses[0]
  );
};

const hasResponseContent = (response: ResponseContract): boolean =>
  Boolean(response.contents?.length);

const isSuccessfulResponse = (response: ResponseContract): boolean =>
  /^2(?:\d\d|XX)$/i.test(response.status);

const isSuccessfulResponseWithContent = (
  response: ResponseContract,
): boolean => isSuccessfulResponse(response) && hasResponseContent(response);
