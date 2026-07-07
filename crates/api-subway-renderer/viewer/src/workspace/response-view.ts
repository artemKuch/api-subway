import type { JsonValue } from "../schema-simulator";
import type {
  ApiSchema,
  Endpoint,
  ResponseContract,
} from "../types";
import type { ExecutionResult } from "../virtual-backend/types";
import { isJsonObject } from "../virtual-backend/types";
import { escapeHtml, prettyJson } from "./html";
import {
  schemaConstraintSummary,
  schemaTypeLabel,
} from "./schema-fields";

const MAX_RENDERED_RESPONSE_ITEMS = 20;
const MAX_RESPONSE_DEPTH = 4;
const MAX_RENDERED_RESPONSE_FIELDS = 500;

interface RenderBudget {
  remaining: number;
}

export const renderResponseSchemaView = (
  endpoint: Endpoint,
  preferredStatus: string,
  response: ExecutionResult | undefined,
  schemas: Map<string, ApiSchema>,
): string => {
  const responses = endpoint.contract?.responses ?? [];
  const contract = selectResponseContract(
    responses,
    preferredStatus,
    response?.status,
  );
  const content = contract?.contents?.[0];
  const alternatives = contract
    ? responses.filter((candidate) => candidate.status !== contract.status)
    : responses;

  if (!contract || !content) {
    return `<section class="response-schema-card unresolved"><strong>${response ? `No response schema declared for ${escapeHtml(response.status)}` : "No response body schema was proven"}</strong>${renderAlternatives(alternatives)}</section>`;
  }

  const schema = schemas.get(content.schema_id);
  if (!schema) {
    return `<section class="response-schema-card unresolved"><strong>Response schema could not be resolved</strong><code>${escapeHtml(content.schema_id)}</code>${renderAlternatives(alternatives)}</section>`;
  }

  return `<section class="body-schema-card response-schema-card"><header class="body-schema-heading"><div><span>Response · ${escapeHtml(contract.status)} · ${escapeHtml(content.media_type)}</span><strong>${escapeHtml(schema.name ?? "Response schema")}</strong><small>Read-only values shaped by this endpoint contract</small></div><code>${escapeHtml(schemaTypeLabel(content.schema_id, schemas))}</code></header>${renderResponseBody(content.schema_id, response?.body, Boolean(response), schemas)}${renderAlternatives(alternatives)}</section>`;
};

export const renderResponseJsonView = (
  response: ExecutionResult | undefined,
): string =>
  `<div class="response-code"><pre><code>${escapeHtml(prettyJson(response?.body ?? { message: "Run the virtual endpoint to see its response" }))}</code></pre></div>`;

const renderResponseBody = (
  schemaId: string,
  value: JsonValue | undefined,
  resolved: boolean,
  schemas: Map<string, ApiSchema>,
): string => {
  const schema = schemas.get(schemaId);
  if (!schema) return '<p class="schema-unresolved">Unresolved schema</p>';
  const budget: RenderBudget = { remaining: MAX_RENDERED_RESPONSE_FIELDS };
  if (schema.kind === "object") {
    const record = value !== undefined && isJsonObject(value) ? value : {};
    const fields = (schema.properties ?? [])
      .slice(0, MAX_RENDERED_RESPONSE_FIELDS)
      .map((property) =>
        renderReadonlyField(
          property.name,
          property.schema_id,
          property.required,
          record[property.name],
          [property.name],
          resolved,
          schemas,
          0,
          new Set([schemaId]),
          budget,
        ),
      )
      .join("");
    return `<div class="body-schema-fields response-schema-fields">${fields || '<p class="schema-unresolved">This response object has no declared fields.</p>'}</div>`;
  }
  if (schema.kind === "array") {
    return `<div class="body-schema-fields response-schema-fields">${renderArrayValue(schema, value, [], resolved, schemas, 0, new Set([schemaId]), budget)}</div>`;
  }
  return `<div class="body-schema-fields response-schema-fields">${renderReadonlyField("response", schemaId, true, value, [], resolved, schemas, 0, new Set(), budget)}</div>`;
};

const renderReadonlyField = (
  name: string,
  schemaId: string,
  required: boolean,
  value: JsonValue | undefined,
  path: string[],
  resolved: boolean,
  schemas: Map<string, ApiSchema>,
  depth: number,
  active: Set<string>,
  budget: RenderBudget,
): string => {
  if (budget.remaining <= 0) return "";
  budget.remaining -= 1;
  const schema = schemas.get(schemaId);
  const typeLabel = schemaTypeLabel(schemaId, schemas);
  if (
    schema?.kind === "object" &&
    depth < MAX_RESPONSE_DEPTH &&
    !active.has(schemaId) &&
    (!resolved || value !== undefined)
  ) {
    const record = value !== undefined && isJsonObject(value) ? value : {};
    const nextActive = new Set(active).add(schemaId);
    return `<fieldset class="nested-schema-group response-schema-group"><legend>${fieldHeading(name, typeLabel, required, schema)}</legend>${(schema.properties ?? []).slice(0, MAX_RENDERED_RESPONSE_FIELDS).map((property) => renderReadonlyField(property.name, property.schema_id, property.required, record[property.name], [...path, property.name], resolved, schemas, depth + 1, nextActive, budget)).join("")}</fieldset>`;
  }
  if (
    schema?.kind === "array" &&
    depth < MAX_RESPONSE_DEPTH &&
    !active.has(schemaId) &&
    (!resolved || value !== undefined)
  ) {
    return `<fieldset class="nested-schema-group response-schema-group response-array-group"><legend>${fieldHeading(name, typeLabel, required, schema)}</legend>${renderArrayValue(schema, value, path, resolved, schemas, depth + 1, new Set(active).add(schemaId), budget)}</fieldset>`;
  }
  return `<div class="body-schema-field response-schema-field ${value === undefined ? "missing" : ""}"><span>${fieldHeading(name, typeLabel, required, schema)}</span><output class="response-field-value" data-response-field-path="${escapeHtml(toPointer(path))}" aria-label="${escapeHtml(`response.${path.join(".") || name}`)}"><code>${escapeHtml(displayValue(value, resolved))}</code></output></div>`;
};

const renderArrayValue = (
  schema: ApiSchema,
  value: JsonValue | undefined,
  path: string[],
  resolved: boolean,
  schemas: Map<string, ApiSchema>,
  depth: number,
  active: Set<string>,
  budget: RenderBudget,
): string => {
  const items = Array.isArray(value) ? value : [];
  const visibleItems = items.slice(0, MAX_RENDERED_RESPONSE_ITEMS);
  const itemSchemaId = schema.items;
  if (!itemSchemaId) {
    return renderRawValue("items", value, path, resolved, "array");
  }
  if (resolved && items.length === 0) {
    return '<div class="response-array-empty">Empty array · 0 items</div>';
  }
  const templateItems: Array<JsonValue | undefined> = resolved
    ? visibleItems
    : [undefined];
  return `<div class="response-array-block"><header><strong>${resolved ? `${items.length} ${items.length === 1 ? "item" : "items"}` : "Item schema"}</strong><code>${escapeHtml(schemaTypeLabel(schema.id, schemas))}</code></header><div class="response-array-items">${templateItems.map((item, index) => renderArrayItem(itemSchemaId, item, [...path, String(index)], index, resolved, schemas, depth, active, budget)).join("")}</div>${items.length > MAX_RENDERED_RESPONSE_ITEMS ? `<p class="response-array-more">${items.length - MAX_RENDERED_RESPONSE_ITEMS} more items · switch to JSON to inspect all</p>` : ""}</div>`;
};

const renderArrayItem = (
  schemaId: string,
  value: JsonValue | undefined,
  path: string[],
  index: number,
  resolved: boolean,
  schemas: Map<string, ApiSchema>,
  depth: number,
  active: Set<string>,
  budget: RenderBudget,
): string => {
  const schema = schemas.get(schemaId);
  if (schema?.kind !== "object" || depth >= MAX_RESPONSE_DEPTH) {
    return renderReadonlyField(
      `Item ${index + 1}`,
      schemaId,
      true,
      value,
      path,
      resolved,
      schemas,
      depth,
      active,
      budget,
    );
  }
  const record = value !== undefined && isJsonObject(value) ? value : {};
  const nextActive = new Set(active).add(schemaId);
  return `<fieldset class="response-array-item"><legend><strong>${resolved ? `Item ${index + 1}` : "Item"}</strong><code>${escapeHtml(schemaTypeLabel(schemaId, schemas))}</code></legend><div class="body-schema-fields response-schema-fields">${(schema.properties ?? []).slice(0, MAX_RENDERED_RESPONSE_FIELDS).map((property) => renderReadonlyField(property.name, property.schema_id, property.required, record[property.name], [...path, property.name], resolved, schemas, depth + 1, nextActive, budget)).join("")}</div></fieldset>`;
};

const renderRawValue = (
  name: string,
  value: JsonValue | undefined,
  path: string[],
  resolved: boolean,
  typeLabel: string,
): string =>
  `<div class="body-schema-field response-schema-field"><span><strong>${escapeHtml(name)}</strong><code>${escapeHtml(typeLabel)}</code></span><output class="response-field-value multiline" data-response-field-path="${escapeHtml(toPointer(path))}"><code>${escapeHtml(displayValue(value, resolved))}</code></output></div>`;

const fieldHeading = (
  name: string,
  typeLabel: string,
  required: boolean,
  schema: ApiSchema | undefined,
): string => {
  const constraints = schemaConstraintSummary(schema);
  return `<strong>${escapeHtml(name)}${required ? " *" : ""}</strong><code>${escapeHtml(typeLabel)}</code>${constraints ? `<small>${escapeHtml(constraints)}</small>` : ""}`;
};

const displayValue = (
  value: JsonValue | undefined,
  resolved: boolean,
): string => {
  if (value === undefined) return resolved ? "Not returned" : "Run endpoint";
  if (typeof value === "string") return value;
  if (value === null || typeof value === "number" || typeof value === "boolean") {
    return String(value);
  }
  return prettyJson(value);
};

const selectResponseContract = (
  responses: ResponseContract[],
  preferredStatus: string,
  actualStatus: string | undefined,
): ResponseContract | undefined => {
  if (actualStatus) {
    return (
      responses.find((response) => response.status === actualStatus) ??
      responses.find((response) => response.status === "default")
    );
  }
  return (
    responses.find(
      (response) =>
        response.status === preferredStatus && response.contents?.length,
    ) ??
    responses.find(
      (response) => /^2\d\d$/.test(response.status) && response.contents?.length,
    ) ??
    responses.find((response) => response.contents?.length) ??
    responses[0]
  );
};

const renderAlternatives = (responses: ResponseContract[]): string =>
  responses.length > 0
    ? `<div class="response-schema-alternatives"><span>Also declared</span>${responses.map((candidate) => `<code>${escapeHtml(candidate.status)}${candidate.contents?.[0] ? ` · ${escapeHtml(candidate.contents[0].media_type)}` : " · no body"}</code>`).join("")}</div>`
    : "";

const toPointer = (path: string[]): string =>
  path
    .map((token) => token.replace(/~/g, "~0").replace(/\//g, "~1"))
    .map((token) => `/${token}`)
    .join("");
