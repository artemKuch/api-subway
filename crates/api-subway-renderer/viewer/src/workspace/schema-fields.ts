import type { JsonValue } from "../schema-simulator";
import type { ApiSchema, SchemaLiteral } from "../types";
import { isJsonObject } from "../virtual-backend/types";
import { escapeHtml, prettyJson } from "./html";

export type BodyFieldKind =
  | "string"
  | "integer"
  | "number"
  | "boolean"
  | "json"
  | "enum";

const MAX_RENDERED_SCHEMA_FIELDS = 500;
const MAX_SCHEMA_LABEL_DEPTH = 8;
const MAX_JSON_VALIDATION_NODES = 10_000;

interface RenderBudget {
  remaining: number;
}

export const renderRequestBodyFields = (
  value: JsonValue,
  schemaId: string,
  schemas: Map<string, ApiSchema>,
  mediaType: string,
): string => {
  const schema = schemas.get(schemaId);
  if (!schema) {
    return '<div class="schema-unresolved">The request body schema could not be resolved.</div>';
  }
  const budget: RenderBudget = { remaining: MAX_RENDERED_SCHEMA_FIELDS };
  const fields =
    schema.kind === "object"
      ? (schema.properties ?? [])
          .slice(0, MAX_RENDERED_SCHEMA_FIELDS)
          .map((property) =>
            renderField(
              property.name,
              property.schema_id,
              property.required,
              isJsonObject(value) ? value[property.name] : undefined,
              [property.name],
              schemas,
              0,
              new Set([schemaId]),
              budget,
            ),
          )
          .join("")
      : renderField(
          "Body",
          schemaId,
          true,
          value,
          [],
          schemas,
          0,
          new Set(),
          budget,
        );
  return `<section class="body-schema-card"><header class="body-schema-heading"><div><span>Body · ${escapeHtml(mediaType)}</span><strong>${escapeHtml(schema.name ?? "Request schema")}</strong><small>Fill the typed fields — Body JSON is built automatically</small></div><code>${escapeHtml(schemaTypeLabel(schemaId, schemas))}</code></header><div class="body-schema-fields">${fields || '<p class="schema-unresolved">This object schema has no declared fields.</p>'}</div></section>`;
};

export const schemaTypeLabel = (
  schemaId: string,
  schemas: Map<string, ApiSchema>,
  depth = 0,
  active = new Set<string>(),
): string => {
  const schema = schemas.get(schemaId);
  if (!schema) return "unresolved";
  if (depth >= MAX_SCHEMA_LABEL_DEPTH || active.has(schemaId)) {
    return schema.kind;
  }
  if (schema.kind === "array") {
    return schema.items
      ? `array<${schemaTypeLabel(
          schema.items,
          schemas,
          depth + 1,
          new Set(active).add(schemaId),
        )}>`
      : "array";
  }
  if (schema.enum_values?.length) return `${schema.kind} · enum`;
  return schema.format ? `${schema.kind} · ${schema.format}` : schema.kind;
};

export const parseBodyFieldValue = (
  rawValue: string,
  kind: BodyFieldKind,
): JsonValue => {
  switch (kind) {
    case "string":
      return rawValue;
    case "integer": {
      if (!rawValue.trim()) throw new Error("Expected an integer");
      const value = Number(rawValue);
      if (!Number.isInteger(value)) throw new Error("Expected an integer");
      return value;
    }
    case "number": {
      if (!rawValue.trim()) throw new Error("Expected a number");
      const value = Number(rawValue);
      if (!Number.isFinite(value)) throw new Error("Expected a finite number");
      return value;
    }
    case "boolean":
      return rawValue === "true";
    case "enum":
    case "json": {
      const parsed: unknown = JSON.parse(rawValue);
      if (!isBoundedJsonValue(parsed, 0, { remaining: MAX_JSON_VALIDATION_NODES })) {
        throw new Error("Expected bounded JSON");
      }
      return parsed;
    }
  }
};

export const updateBodyAtPointer = (
  body: JsonValue,
  pointer: string,
  value: JsonValue | undefined,
): JsonValue => {
  if (!pointer) return value ?? null;
  const tokens = pointer
    .split("/")
    .slice(1)
    .map((token) => token.replace(/~1/g, "/").replace(/~0/g, "~"));
  const root: { [key: string]: JsonValue } = isJsonObject(body)
    ? structuredClone(body)
    : {};
  let current = root;
  for (const token of tokens.slice(0, -1)) {
    const next = Object.hasOwn(current, token) ? current[token] : undefined;
    if (next !== undefined && isJsonObject(next)) {
      current = next;
    } else {
      const created: { [key: string]: JsonValue } = {};
      defineJsonProperty(current, token, created);
      current = created;
    }
  }
  const finalToken = tokens.at(-1);
  if (!finalToken) return root;
  if (value === undefined) {
    delete current[finalToken];
  } else {
    defineJsonProperty(current, finalToken, value);
  }
  return root;
};

const defineJsonProperty = (
  target: { [key: string]: JsonValue },
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

const renderField = (
  name: string,
  schemaId: string,
  required: boolean,
  value: JsonValue | undefined,
  path: string[],
  schemas: Map<string, ApiSchema>,
  depth: number,
  active: Set<string>,
  budget: RenderBudget,
): string => {
  if (budget.remaining <= 0) return "";
  budget.remaining -= 1;
  const schema = schemas.get(schemaId);
  const typeLabel = schemaTypeLabel(schemaId, schemas);
  if (!schema) return renderJsonField(name, required, value, path, typeLabel);
  if (schema.kind === "object" && depth < 3 && !active.has(schemaId)) {
    const record = value !== undefined && isJsonObject(value) ? value : {};
    const nextActive = new Set(active).add(schemaId);
    return `<fieldset class="nested-schema-group"><legend>${fieldHeading(name, typeLabel, required, schema)}</legend>${(schema.properties ?? []).slice(0, MAX_RENDERED_SCHEMA_FIELDS).map((property) => renderField(property.name, property.schema_id, property.required, record[property.name], [...path, property.name], schemas, depth + 1, nextActive, budget)).join("")}</fieldset>`;
  }
  if (schema.enum_values?.length) {
    const selected = JSON.stringify(value ?? literalValue(schema.enum_values[0]!));
      return `<label class="body-schema-field"><span>${fieldHeading(name, typeLabel, required, schema)}</span><select aria-label="${escapeHtml(fieldAriaLabel(path, name))}" data-body-field-path="${escapeHtml(toPointer(path))}" data-body-field-kind="enum" data-body-field-required="${required}">${schema.enum_values.slice(0, MAX_RENDERED_SCHEMA_FIELDS).map((literal) => {
      const optionValue = JSON.stringify(literalValue(literal));
      return `<option value="${escapeHtml(optionValue)}" ${optionValue === selected ? "selected" : ""}>${escapeHtml(String(literalValue(literal)))}</option>`;
    }).join("")}</select></label>`;
  }
  switch (schema.kind) {
    case "string":
      return renderInputField(
        name,
        required,
        typeof value === "string" ? value : "",
        path,
        "string",
        typeLabel,
        schema,
        inputType(schema.format),
      );
    case "integer":
    case "number":
      return renderInputField(
        name,
        required,
        typeof value === "number" ? String(value) : "",
        path,
        schema.kind,
        typeLabel,
        schema,
        "number",
      );
    case "boolean":
      return `<label class="body-schema-field"><span>${fieldHeading(name, typeLabel, required, schema)}</span><select aria-label="${escapeHtml(fieldAriaLabel(path, name))}" data-body-field-path="${escapeHtml(toPointer(path))}" data-body-field-kind="boolean" data-body-field-required="${required}"><option value="true" ${value === true ? "selected" : ""}>true</option><option value="false" ${value === false ? "selected" : ""}>false</option></select></label>`;
    case "null":
      return `<div class="body-schema-field readonly"><span>${fieldHeading(name, typeLabel, required, schema)}</span><code>null</code></div>`;
    default:
      return renderJsonField(name, required, value, path, typeLabel, schema);
  }
};

const renderInputField = (
  name: string,
  required: boolean,
  value: string,
  path: string[],
  kind: "string" | "integer" | "number",
  typeLabel: string,
  schema: ApiSchema,
  type: string,
): string =>
  `<label class="body-schema-field"><span>${fieldHeading(name, typeLabel, required, schema)}</span><input aria-label="${escapeHtml(fieldAriaLabel(path, name))}" type="${type}" value="${escapeHtml(value)}" data-body-field-path="${escapeHtml(toPointer(path))}" data-body-field-kind="${kind}" data-body-field-required="${required}" ${required ? "required" : ""} ${kind === "integer" ? 'step="1"' : ""} autocomplete="off" spellcheck="false"></label>`;

const renderJsonField = (
  name: string,
  required: boolean,
  value: JsonValue | undefined,
  path: string[],
  typeLabel: string,
  schema?: ApiSchema,
): string =>
  `<label class="body-schema-field body-schema-json-field"><span>${fieldHeading(name, typeLabel, required, schema)}</span><textarea aria-label="${escapeHtml(fieldAriaLabel(path, name))}" data-body-field-path="${escapeHtml(toPointer(path))}" data-body-field-kind="json" data-body-field-required="${required}" spellcheck="false">${escapeHtml(prettyJson(value ?? null))}</textarea></label>`;

const fieldHeading = (
  name: string,
  typeLabel: string,
  required: boolean,
  schema?: ApiSchema,
): string =>
  `<strong>${escapeHtml(name)}${required ? " *" : ""}</strong><code>${escapeHtml(typeLabel)}</code>${schemaConstraintSummary(schema) ? `<small>${escapeHtml(schemaConstraintSummary(schema))}</small>` : ""}`;

export const schemaConstraintSummary = (schema?: ApiSchema): string => {
  if (!schema) return "";
  if (schema.enum_values?.length) {
    const visible = schema.enum_values.slice(0, 50);
    const remaining = schema.enum_values.length - visible.length;
    return `one of ${visible.map((literal) => String(literalValue(literal))).join(", ")}${remaining > 0 ? `, +${remaining} more` : ""}`;
  }
  const constraints = schema.constraints;
  if (!constraints) return schema.nullable ? "nullable" : "";
  const parts: string[] = [];
  if (constraints.min_length !== undefined) parts.push(`min ${constraints.min_length} chars`);
  if (constraints.max_length !== undefined) parts.push(`max ${constraints.max_length} chars`);
  if (constraints.minimum !== undefined) parts.push(`min ${constraints.minimum}`);
  if (constraints.maximum !== undefined) parts.push(`max ${constraints.maximum}`);
  if (constraints.min_items !== undefined) parts.push(`min ${constraints.min_items} items`);
  if (constraints.max_items !== undefined) parts.push(`max ${constraints.max_items} items`);
  if (schema.nullable) parts.push("nullable");
  return parts.join(" · ");
};

const toPointer = (path: string[]): string =>
  path
    .map((token) => token.replace(/~/g, "~0").replace(/\//g, "~1"))
    .map((token) => `/${token}`)
    .join("");

const fieldAriaLabel = (path: string[], fallback: string): string =>
  path.length > 0 ? path.join(".") : fallback;

const inputType = (format: string | undefined): string => {
  if (format === "email") return "email";
  if (format === "date") return "date";
  if (format === "uri" || format === "url") return "url";
  return "text";
};

const literalValue = (literal: SchemaLiteral): JsonValue => {
  switch (literal.kind) {
    case "string":
      return literal.value;
    case "integer":
    case "number":
      return Number(literal.value);
    case "boolean":
      return literal.value === "true";
    case "null":
      return null;
  }
};

const isBoundedJsonValue = (
  value: unknown,
  depth: number,
  budget: { remaining: number },
): value is JsonValue => {
  if (depth > 20 || budget.remaining <= 0) return false;
  budget.remaining -= 1;
  if (
    value === null ||
    typeof value === "string" ||
    typeof value === "boolean"
  ) {
    return true;
  }
  if (typeof value === "number") return Number.isFinite(value);
  if (Array.isArray(value)) {
    return (
      value.length <= 2_000 &&
      value.every((item) => isBoundedJsonValue(item, depth + 1, budget))
    );
  }
  if (typeof value !== "object") return false;
  const entries = Object.entries(value);
  return (
    entries.length <= 300 &&
    entries.every(([, item]) => isBoundedJsonValue(item, depth + 1, budget))
  );
};
