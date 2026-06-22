import type {
  ApiSchema,
  SchemaLiteral,
} from "./types";

export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

const MAX_OBJECT_PROPERTIES = 100;
const MAX_ARRAY_ITEMS = 20;
const MAX_VALIDATION_ARRAY_ITEMS = 2_000;
const MAX_SCHEMA_VARIANTS = 20;
const MAX_STRING_LENGTH = 512;
const MAX_PATTERN_LENGTH = 128;
const MAX_SIMULATION_WORK = 10_000;

interface WorkBudget {
  remaining: number;
}

export const createSchemaIndex = (
  schemas: ApiSchema[] = [],
): Map<string, ApiSchema> =>
  new Map(schemas.map((schema) => [schema.id, schema]));

export const generateValue = (
  schemaId: string,
  schemas: Map<string, ApiSchema>,
  random: () => number,
  depth = 0,
  active = new Set<string>(),
  budget: WorkBudget = { remaining: MAX_SIMULATION_WORK },
): JsonValue => {
  const schema = schemas.get(schemaId);
  if (!schema) return null;
  if (budget.remaining <= 0) return terminalValue(schema, random);
  budget.remaining -= 1;
  if (schema.const_value) return literalValue(schema.const_value);
  if (schema.enum_values?.length) {
    const selected = schema.enum_values[
      Math.floor(random() * schema.enum_values.length)
    ];
    return selected ? literalValue(selected) : null;
  }
  if (depth >= 7 || active.has(schemaId)) return terminalValue(schema, random);
  const nextActive = new Set(active).add(schemaId);
  switch (schema.kind) {
    case "object":
      return Object.fromEntries(
        (schema.properties ?? []).slice(0, MAX_OBJECT_PROPERTIES).map((property) => [
          property.name,
          generateValue(
            property.schema_id,
            schemas,
            random,
            depth + 1,
            nextActive,
            budget,
          ),
        ]),
      );
    case "array": {
      const minimum = schema.constraints?.min_items ?? 0;
      const maximum = schema.constraints?.max_items ?? 3;
      const count = Math.min(
        MAX_ARRAY_ITEMS,
        Math.max(minimum, Math.min(maximum, 2)),
      );
      return Array.from({ length: count }, () =>
        schema.items
          ? generateValue(
              schema.items,
              schemas,
              random,
              depth + 1,
              nextActive,
              budget,
            )
          : null,
      );
    }
    case "union": {
      const variants = (schema.variants ?? []).slice(0, MAX_SCHEMA_VARIANTS);
      const selected = variants[Math.floor(random() * variants.length)];
      return selected
        ? generateValue(
            selected,
            schemas,
            random,
            depth + 1,
            nextActive,
            budget,
          )
        : null;
    }
    case "intersection":
      return (schema.variants ?? [])
        .slice(0, MAX_SCHEMA_VARIANTS)
        .reduce<JsonValue>((combined, variant) => {
          const value = generateValue(
            variant,
            schemas,
            random,
            depth + 1,
            nextActive,
            budget,
          );
          return isRecord(combined) && isRecord(value)
            ? mergeGeneratedRecords(combined, value)
            : value;
        }, {});
    default:
      return terminalValue(schema, random);
  }
};

export const validateValue = (
  schemaId: string,
  value: JsonValue,
  schemas: Map<string, ApiSchema>,
  path = "$",
  depth = 0,
  budget: WorkBudget = { remaining: MAX_SIMULATION_WORK },
): string[] => {
  const schema = schemas.get(schemaId);
  if (!schema) return [`${path} references an unresolved schema`];
  if (budget.remaining <= 0) {
    return [`${path} exceeds the simulator work budget`];
  }
  budget.remaining -= 1;
  if (value === null && schema.nullable) return [];
  if (depth >= 10) {
    return [`${path} exceeds the simulator validation depth`];
  }
  const errors: string[] = [];
  if (schema.const_value && !sameValue(value, literalValue(schema.const_value))) {
    errors.push(`${path} does not match the declared constant`);
  }
  if (
    schema.enum_values?.length &&
    !schema.enum_values.some((item) => sameValue(value, literalValue(item)))
  ) {
    errors.push(`${path} is not one of the declared enum values`);
  }
  switch (schema.kind) {
    case "object":
      if (!isRecord(value)) return [`${path} must be an object`];
      if ((schema.properties?.length ?? 0) > MAX_OBJECT_PROPERTIES) {
        errors.push(
          `${path} exceeds the ${MAX_OBJECT_PROPERTIES}-property simulator budget`,
        );
      }
      for (const property of (schema.properties ?? []).slice(
        0,
        MAX_OBJECT_PROPERTIES,
      )) {
        if (!Object.hasOwn(value, property.name)) {
          if (property.required) errors.push(`${path}.${property.name} is required`);
          continue;
        }
        errors.push(
          ...validateValue(
            property.schema_id,
            value[property.name]!,
            schemas,
            `${path}.${property.name}`,
            depth + 1,
            budget,
          ),
        );
      }
      break;
    case "array":
      if (!Array.isArray(value)) return [`${path} must be an array`];
      if (value.length < (schema.constraints?.min_items ?? 0)) {
        errors.push(`${path} has too few items`);
      }
      if (value.length > (schema.constraints?.max_items ?? Infinity)) {
        errors.push(`${path} has too many items`);
      }
      if (value.length > MAX_VALIDATION_ARRAY_ITEMS) {
        errors.push(
          `${path} exceeds the ${MAX_VALIDATION_ARRAY_ITEMS}-item simulator validation budget`,
        );
      }
      if (schema.items) {
        value.slice(0, MAX_VALIDATION_ARRAY_ITEMS).forEach((item, index) =>
          errors.push(
            ...validateValue(
              schema.items!,
              item,
              schemas,
              `${path}[${index}]`,
              depth + 1,
              budget,
            ),
          ),
        );
      }
      break;
    case "string":
      if (typeof value !== "string") return [`${path} must be a string`];
      validateString(schema, value, path, errors);
      break;
    case "integer":
      if (typeof value !== "number" || !Number.isInteger(value)) {
        return [`${path} must be an integer`];
      }
      validateNumber(schema, value, path, errors);
      break;
    case "number":
      if (typeof value !== "number" || !Number.isFinite(value)) {
        return [`${path} must be a number`];
      }
      validateNumber(schema, value, path, errors);
      break;
    case "boolean":
      if (typeof value !== "boolean") errors.push(`${path} must be a boolean`);
      break;
    case "null":
      if (value !== null) errors.push(`${path} must be null`);
      break;
    case "union": {
      const variants = (schema.variants ?? []).slice(0, MAX_SCHEMA_VARIANTS);
      if ((schema.variants?.length ?? 0) > MAX_SCHEMA_VARIANTS) {
        errors.push(`${path} exceeds the schema-variant simulator budget`);
      }
      if (
        variants.length > 0 &&
        !variants.some(
          (variant) =>
            validateValue(variant, value, schemas, path, depth + 1, budget)
              .length === 0,
        )
      ) {
        errors.push(`${path} does not match any schema variant`);
      }
      break;
    }
    case "intersection":
      if ((schema.variants?.length ?? 0) > MAX_SCHEMA_VARIANTS) {
        errors.push(`${path} exceeds the schema-variant simulator budget`);
      }
      for (const variant of (schema.variants ?? []).slice(
        0,
        MAX_SCHEMA_VARIANTS,
      )) {
        errors.push(
          ...validateValue(
            variant,
            value,
            schemas,
            path,
            depth + 1,
            budget,
          ),
        );
      }
      break;
    case "unknown":
      break;
  }
  return errors;
};

const terminalValue = (schema: ApiSchema, random: () => number): JsonValue => {
  switch (schema.kind) {
    case "string":
      return generatedString(schema, random);
    case "integer":
      return generatedNumber(schema, true);
    case "number":
      return generatedNumber(schema, false);
    case "boolean":
      return random() >= 0.5;
    case "null":
      return null;
    case "object":
    case "intersection":
      return {};
    case "array":
      return [];
    case "union":
    case "unknown":
      return null;
  }
};

const generatedString = (schema: ApiSchema, random: () => number): string => {
  const suffix = Math.floor(random() * 10_000).toString().padStart(4, "0");
  let value = (() => {
    switch (schema.format) {
      case "email":
        return `user-${suffix}@example.com`;
      case "uuid":
        return generatedUuid(random);
      case "date-time":
        return "2026-01-15T10:30:00Z";
      case "date":
        return "2026-01-15";
      case "uri":
      case "url":
        return `https://example.com/resource-${suffix}`;
      case "ipv4":
        return "192.0.2.1";
      default:
        return `sample-${suffix}`;
    }
  })();
  const minimum = Math.min(
    schema.constraints?.min_length ?? 0,
    MAX_STRING_LENGTH,
  );
  if (value.length < minimum) value = value.padEnd(minimum, "x");
  const maximum = schema.constraints?.max_length;
  if (maximum !== undefined && value.length > maximum) {
    value = value.slice(0, Math.min(maximum, MAX_STRING_LENGTH));
  }
  if (value.length > MAX_STRING_LENGTH) value = value.slice(0, MAX_STRING_LENGTH);
  return value;
};

const generatedNumber = (schema: ApiSchema, integer: boolean): number => {
  const minimum = Number(schema.constraints?.minimum ?? (integer ? 1 : 1.5));
  const maximum = Number(schema.constraints?.maximum ?? minimum + 100);
  const safeMinimum = Number.isFinite(minimum) ? minimum : 1;
  const safeMaximum = Number.isFinite(maximum) ? maximum : safeMinimum + 100;
  const value = Math.min(safeMaximum, safeMinimum + (integer ? 1 : 0.5));
  return integer ? Math.round(value) : value;
};

const generatedUuid = (random: () => number): string => {
  const bytes = Array.from({ length: 16 }, () => Math.floor(random() * 256));
  bytes[6] = ((bytes[6] ?? 0) & 0x0f) | 0x40;
  bytes[8] = ((bytes[8] ?? 0) & 0x3f) | 0x80;
  const hex = bytes.map((value) => value.toString(16).padStart(2, "0"));
  return `${hex.slice(0, 4).join("")}-${hex.slice(4, 6).join("")}-${hex.slice(6, 8).join("")}-${hex.slice(8, 10).join("")}-${hex.slice(10).join("")}`;
};

const validateString = (
  schema: ApiSchema,
  value: string,
  path: string,
  errors: string[],
): void => {
  if (value.length < (schema.constraints?.min_length ?? 0)) {
    errors.push(`${path} is shorter than minLength`);
  }
  if (value.length > (schema.constraints?.max_length ?? Infinity)) {
    errors.push(`${path} is longer than maxLength`);
  }
  if (schema.constraints?.pattern) {
    if (!isSafePattern(schema.constraints.pattern)) {
      errors.push(`${path} uses a pattern outside the safe simulator subset`);
    } else {
      try {
        if (!new RegExp(schema.constraints.pattern).test(value)) {
          errors.push(`${path} does not match the declared pattern`);
        }
      } catch {
        errors.push(`${path} uses a pattern unsupported by this browser`);
      }
    }
  }
  const formatValidator = formatValidators[schema.format ?? ""];
  if (formatValidator && !formatValidator(value)) {
    errors.push(`${path} must match the ${schema.format} format`);
  }
};

const isSafePattern = (pattern: string): boolean => {
  if (pattern.length > MAX_PATTERN_LENGTH) return false;
  let escaped = false;
  let characterClass = false;
  for (let index = 0; index < pattern.length; index += 1) {
    const character = pattern[index];
    if (escaped) {
      if (character && /[1-9]/.test(character)) return false;
      escaped = false;
      continue;
    }
    if (character === "\\") {
      escaped = true;
      continue;
    }
    if (character === "[" && !characterClass) {
      characterClass = true;
      continue;
    }
    if (character === "]" && characterClass) {
      characterClass = false;
      continue;
    }
    if (characterClass) continue;
    if (character === "(" || character === ")" || character === "|") {
      return false;
    }
    if (character === "+" || character === "*" || character === "?") {
      return false;
    }
    if (character === "{") {
      const close = pattern.indexOf("}", index + 1);
      if (close < 0) return false;
      const repetitions = pattern.slice(index + 1, close);
      if (!/^\d+$/.test(repetitions) || Number(repetitions) > 256) {
        return false;
      }
      index = close;
    } else if (character === "}") {
      return false;
    }
  }
  return !escaped && !characterClass;
};

const validateNumber = (
  schema: ApiSchema,
  value: number,
  path: string,
  errors: string[],
): void => {
  const minimum = Number(schema.constraints?.minimum ?? -Infinity);
  const maximum = Number(schema.constraints?.maximum ?? Infinity);
  if (value < minimum) errors.push(`${path} is below minimum`);
  if (value > maximum) errors.push(`${path} is above maximum`);
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

const sameValue = (left: JsonValue, right: JsonValue): boolean =>
  JSON.stringify(left) === JSON.stringify(right);

const isRecord = (value: JsonValue): value is Record<string, JsonValue> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const mergeGeneratedRecords = (
  left: Record<string, JsonValue>,
  right: Record<string, JsonValue>,
): Record<string, JsonValue> => {
  const entries = [...Object.entries(left), ...Object.entries(right)].slice(
    -MAX_OBJECT_PROPERTIES,
  );
  return Object.fromEntries(entries);
};

export const seededRandom = (seed: number): (() => number) => {
  let state = seed >>> 0 || 1;
  return () => {
    state += 0x6d2b79f5;
    let value = state;
    value = Math.imul(value ^ (value >>> 15), value | 1);
    value ^= value + Math.imul(value ^ (value >>> 7), value | 61);
    return ((value ^ (value >>> 14)) >>> 0) / 4_294_967_296;
  };
};

const formatValidators: Record<string, (value: string) => boolean> = {
  email: (value) => /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(value),
  uuid: (value) =>
    /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(
      value,
    ),
  date: (value) => {
    const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
    if (!match) return false;
    const year = Number(match[1]);
    const month = Number(match[2]);
    const day = Number(match[3]);
    const date = new Date(Date.UTC(year, month - 1, day));
    return (
      date.getUTCFullYear() === year &&
      date.getUTCMonth() === month - 1 &&
      date.getUTCDate() === day
    );
  },
  "date-time": (value) =>
    /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/.test(value) &&
    Number.isFinite(Date.parse(value)),
  ipv4: (value) => {
    const octets = value.split(".");
    return (
      octets.length === 4 &&
      octets.every(
        (octet) => /^\d{1,3}$/.test(octet) && Number(octet) <= 255,
      )
    );
  },
  uri: isAbsoluteUri,
  url: isAbsoluteUrl,
};

function isAbsoluteUri(value: string): boolean {
  try {
    return Boolean(new URL(value).protocol);
  } catch {
    return false;
  }
}

function isAbsoluteUrl(value: string): boolean {
  try {
    const parsed = new URL(value);
    return /^(?:https?|ftp):$/.test(parsed.protocol) && Boolean(parsed.hostname);
  } catch {
    return false;
  }
}
