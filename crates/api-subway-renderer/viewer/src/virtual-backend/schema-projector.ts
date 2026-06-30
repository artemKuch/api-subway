import {
  generateValue,
  validateValue,
  type JsonValue,
} from "../schema-simulator";
import type { ApiSchema } from "../types";
import { isJsonObject, type JsonObject } from "./types";

const MAX_PROJECTED_PROPERTIES = 100;
const MAX_PROJECTED_VARIANTS = 20;
const MAX_PROJECTION_DEPTH = 10;
const MAX_PROJECTION_WORK = 10_000;

interface ProjectionBudget {
  remaining: number;
}

export const projectValueToSchema = (
  schemaId: string,
  source: JsonValue,
  schemas: Map<string, ApiSchema>,
  random: () => number,
  depth = 0,
  active = new Set<string>(),
  budget: ProjectionBudget = { remaining: MAX_PROJECTION_WORK },
): JsonValue => {
  const schema = schemas.get(schemaId);
  if (!schema) return source;
  if (budget.remaining <= 0) return source;
  budget.remaining -= 1;
  if (source === null && schema.nullable) return null;
  if (depth >= MAX_PROJECTION_DEPTH || active.has(schemaId)) {
    return generateValue(schemaId, schemas, random, depth, active);
  }

  const nextActive = new Set(active).add(schemaId);
  switch (schema.kind) {
    case "object":
      return projectObject(schema, source, schemas, random, depth, nextActive, budget);
    case "array":
      return projectArray(schema, source, schemas, random, depth, nextActive, budget);
    case "union":
      return projectUnion(schema, source, schemas, random, depth, nextActive, budget);
    case "intersection":
      return projectIntersection(
        schema,
        source,
        schemas,
        random,
        depth,
        nextActive,
        budget,
      );
    case "unknown":
      return source;
    default:
      return validateValue(schemaId, source, schemas).length === 0
        ? source
        : generateValue(schemaId, schemas, random, depth, active, budget);
  }
};

const projectObject = (
  schema: ApiSchema,
  source: JsonValue,
  schemas: Map<string, ApiSchema>,
  random: () => number,
  depth: number,
  active: Set<string>,
  budget: ProjectionBudget,
): JsonObject => {
  const sourceObject = isJsonObject(source) ? source : undefined;
  const projected: JsonObject = {};
  for (const property of (schema.properties ?? []).slice(
    0,
    MAX_PROJECTED_PROPERTIES,
  )) {
    const hasSource = Boolean(
      sourceObject && Object.hasOwn(sourceObject, property.name),
    );
    if (!hasSource && !property.required) continue;
    const propertySource = hasSource
      ? sourceObject?.[property.name]
      : generateValue(
          property.schema_id,
          schemas,
          random,
          depth + 1,
          active,
          budget,
        );
    defineJsonProperty(
      projected,
      property.name,
      projectValueToSchema(
        property.schema_id,
        propertySource ?? null,
        schemas,
        random,
        depth + 1,
        active,
        budget,
      ),
    );
  }
  return projected;
};

const projectArray = (
  schema: ApiSchema,
  source: JsonValue,
  schemas: Map<string, ApiSchema>,
  random: () => number,
  depth: number,
  active: Set<string>,
  budget: ProjectionBudget,
): JsonValue => {
  const generated = Array.isArray(source)
    ? source
    : generateValue(schema.id, schemas, random, depth, new Set(), budget);
  if (!Array.isArray(generated) || !schema.items) return generated;
  return generated.map((item) =>
    projectValueToSchema(
      schema.items!,
      item,
      schemas,
      random,
      depth + 1,
      active,
      budget,
    ),
  );
};

const projectUnion = (
  schema: ApiSchema,
  source: JsonValue,
  schemas: Map<string, ApiSchema>,
  random: () => number,
  depth: number,
  active: Set<string>,
  budget: ProjectionBudget,
): JsonValue => {
  const variants = (schema.variants ?? []).slice(0, MAX_PROJECTED_VARIANTS);
  const selected =
    variants.find(
      (variant) =>
        validateValue(variant, source, schemas, "$", 0, budget).length === 0,
    ) ?? variants[0];
  return selected
    ? projectValueToSchema(
        selected,
        source,
        schemas,
        random,
        depth + 1,
        active,
        budget,
      )
    : source;
};

const projectIntersection = (
  schema: ApiSchema,
  source: JsonValue,
  schemas: Map<string, ApiSchema>,
  random: () => number,
  depth: number,
  active: Set<string>,
  budget: ProjectionBudget,
): JsonValue => {
  const projected: JsonObject = {};
  for (const variant of (schema.variants ?? []).slice(
    0,
    MAX_PROJECTED_VARIANTS,
  )) {
    const value = projectValueToSchema(
      variant,
      source,
      schemas,
      random,
      depth + 1,
      active,
      budget,
    );
    if (!isJsonObject(value)) {
      return generateValue(schema.id, schemas, random, depth, new Set(), budget);
    }
    for (const [key, item] of Object.entries(value)) {
      defineJsonProperty(projected, key, item);
    }
  }
  return projected;
};

const defineJsonProperty = (
  target: JsonObject,
  key: string,
  value: JsonValue,
): void => {
  Object.defineProperty(target, key, {
    configurable: true,
    enumerable: true,
    value,
    writable: true,
  });
};
