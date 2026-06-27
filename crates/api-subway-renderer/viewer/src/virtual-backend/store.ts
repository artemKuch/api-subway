import type { JsonValue } from "../schema-simulator";
import {
  isJsonObject,
  type BackendChange,
  type EditableBackendSnapshot,
  type JsonObject,
  type VirtualBackendSnapshot,
  type VirtualResource,
} from "./types";
import {
  createDictionary,
  getDictionaryValue,
  setDictionaryValue,
} from "./dictionary";

type BackendListener = (change: BackendChange) => void;

const MAX_RESOURCES = 200;
const MAX_RECORDS_PER_RESOURCE = 2_000;
const MAX_OBJECT_KEYS = 300;
const MAX_JSON_DEPTH = 20;
const MAX_RESOURCE_NAME_LENGTH = 128;
const MAX_PRIMARY_KEY_LENGTH = 128;
const MAX_JSON_NODES = 100_000;
export const MAX_BACKEND_JSON_BYTES = 2_000_000;

export class VirtualBackendStore {
  private readonly initial: VirtualBackendSnapshot;
  private current: VirtualBackendSnapshot;
  private readonly listeners = new Set<BackendListener>();

  constructor(snapshot: VirtualBackendSnapshot) {
    const editable = parseEditableSnapshot({
      resources: snapshot.resources,
    });
    const validated = {
      ...snapshot,
      resources: editable.resources,
    };
    this.initial = clone(validated);
    this.current = clone(validated);
  }

  snapshot(): VirtualBackendSnapshot {
    return clone(this.current);
  }

  editableSnapshot(): EditableBackendSnapshot {
    return {
      resources: clone(this.current.resources),
    };
  }

  resource(name: string): VirtualResource | undefined {
    const resource = getDictionaryValue(this.current.resources, name);
    return resource ? clone(resource) : undefined;
  }

  operation(endpointId: string) {
    const operation = getDictionaryValue(this.current.operations, endpointId);
    return operation ? clone(operation) : undefined;
  }

  operations() {
    return clone(this.current.operations);
  }

  commitResource(
    resource: string,
    records: JsonObject[],
    endpointId: string,
  ): BackendChange {
    const currentResource = getDictionaryValue(this.current.resources, resource);
    if (!currentResource) throw new Error(`Unknown virtual resource: ${resource}`);
    validateRecords(records, resource, currentResource.primaryKey);
    setDictionaryValue(this.current.resources, resource, {
      primaryKey: currentResource.primaryKey,
      records: clone(records),
    });
    return this.publish({
      endpointId,
      resource,
    });
  }

  replaceEditableSnapshot(value: unknown): BackendChange {
    const parsed = parseEditableSnapshot(value);
    this.current.resources = clone(parsed.resources);
    return this.publish({
      endpointId: "virtual-backend:apply",
      resource: "*",
    });
  }

  reset(): BackendChange {
    this.current.resources = clone(this.initial.resources);
    return this.publish({
      endpointId: "virtual-backend:reset",
      resource: "*",
    });
  }

  subscribe(listener: BackendListener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private publish(change: BackendChange): BackendChange {
    for (const listener of this.listeners) {
      try {
        listener(change);
      } catch (error) {
        queueMicrotask(() => {
          throw error;
        });
      }
    }
    return change;
  }
}

export const parseEditableSnapshot = (
  value: unknown,
): EditableBackendSnapshot => {
  if (!isUnknownRecord(value)) {
    throw new Error("Virtual backend JSON must be an object");
  }
  assertAllowedProperties(value, ["resources"], "Virtual backend");
  if (!isUnknownRecord(value.resources)) {
    throw new Error("Virtual backend JSON must contain a resources object");
  }
  const entries = Object.entries(value.resources);
  if (entries.length > MAX_RESOURCES) {
    throw new Error(`Virtual backend exceeds the ${MAX_RESOURCES}-resource budget`);
  }
  const resources = createDictionary<VirtualResource>();
  const jsonBudget = { remaining: MAX_JSON_NODES };
  for (const [name, candidate] of entries) {
    if (
      name.trim().length === 0 ||
      name.length > MAX_RESOURCE_NAME_LENGTH
    ) {
      throw new Error(
        `Resource names must contain between 1 and ${MAX_RESOURCE_NAME_LENGTH} characters`,
      );
    }
    if (!isUnknownRecord(candidate)) {
      throw new Error(`Resource ${name} must be an object`);
    }
    assertAllowedProperties(candidate, ["primaryKey", "records"], `Resource ${name}`);
    if (
      typeof candidate.primaryKey !== "string" ||
      candidate.primaryKey.trim().length === 0 ||
      candidate.primaryKey.length > MAX_PRIMARY_KEY_LENGTH
    ) {
      throw new Error(
        `Resource ${name} must declare a primaryKey between 1 and ${MAX_PRIMARY_KEY_LENGTH} characters`,
      );
    }
    if (!Array.isArray(candidate.records)) {
      throw new Error(`Resource ${name} must contain a records array`);
    }
    const records: unknown[] = candidate.records;
    validateRecords(records, name, candidate.primaryKey, jsonBudget);
    setDictionaryValue(resources, name, {
      primaryKey: candidate.primaryKey,
      records,
    });
  }
  return {
    resources,
  };
};

export const parseEditableSnapshotText = (
  text: string,
): EditableBackendSnapshot => {
  if (
    text.length > MAX_BACKEND_JSON_BYTES ||
    new TextEncoder().encode(text).byteLength > MAX_BACKEND_JSON_BYTES
  ) {
    throw new Error("Virtual backend JSON is limited to 2 MB");
  }
  const value: unknown = JSON.parse(text);
  return parseEditableSnapshot(value);
};

function validateRecords(
  records: unknown[],
  resource: string,
  primaryKey: string,
  budget = { remaining: MAX_JSON_NODES },
): asserts records is JsonObject[] {
  if (records.length > MAX_RECORDS_PER_RESOURCE) {
    throw new Error(
      `Resource ${resource} exceeds the ${MAX_RECORDS_PER_RESOURCE}-record budget`,
    );
  }
  const identifiers = new Set<string>();
  for (const [index, record] of records.entries()) {
    if (!isJsonValue(record, 0, budget)) {
      throw new Error(`Resource ${resource} record ${index} is not bounded JSON`);
    }
    if (!isJsonObject(record)) {
      throw new Error(`Resource ${resource} record ${index} must be a JSON object`);
    }
    const identifier = Object.hasOwn(record, primaryKey)
      ? record[primaryKey]
      : undefined;
    if (
      !(
        (typeof identifier === "string" && identifier.length > 0) ||
        (typeof identifier === "number" && Number.isFinite(identifier))
      )
    ) {
      throw new Error(
        `Resource ${resource} record ${index} must contain a string or number ${primaryKey}`,
      );
    }
    const identity = String(identifier);
    if (identifiers.has(identity)) {
      throw new Error(`Resource ${resource} contains duplicate ${primaryKey} ${identity}`);
    }
    identifiers.add(identity);
  }
}

const isJsonValue = (
  value: unknown,
  depth: number,
  budget: { remaining: number },
): value is JsonValue => {
  if (depth > MAX_JSON_DEPTH || budget.remaining <= 0) return false;
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
      value.length <= MAX_RECORDS_PER_RESOURCE &&
      value.every((item) => isJsonValue(item, depth + 1, budget))
    );
  }
  if (!isUnknownRecord(value)) return false;
  const entries = Object.entries(value);
  return (
    entries.length <= MAX_OBJECT_KEYS &&
    entries.every(([, item]) => isJsonValue(item, depth + 1, budget))
  );
};

const isUnknownRecord = (value: unknown): value is Record<string, unknown> => {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return false;
  }
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
};

const assertAllowedProperties = (
  value: Record<string, unknown>,
  allowed: readonly string[],
  scope: string,
): void => {
  const unexpected = Object.keys(value).find((key) => !allowed.includes(key));
  if (unexpected) {
    throw new Error(`${scope} contains unsupported property ${unexpected}`);
  }
};

const clone = <T>(value: T): T => structuredClone(value);
