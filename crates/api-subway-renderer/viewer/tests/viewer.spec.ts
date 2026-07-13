import { expect, test, type Page } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { fileURLToPath, pathToFileURL } from "node:url";
import path from "node:path";
import {
  createSchemaIndex,
  generateValue,
  type JsonValue,
  validateValue,
} from "../src/schema-simulator";
import type { ApiMap, ApiSchema, Endpoint } from "../src/types";
import { VirtualBackendEngine } from "../src/virtual-backend/engine";
import { planVirtualBackend } from "../src/virtual-backend/planner";
import { projectValueToSchema } from "../src/virtual-backend/schema-projector";
import {
  parseEditableSnapshot,
  parseEditableSnapshotText,
  VirtualBackendStore,
} from "../src/virtual-backend/store";
import {
  parseBodyFieldValue,
  updateBodyAtPointer,
} from "../src/workspace/schema-fields";

const artifact =
  process.env.API_SUBWAY_HTML ??
  path.resolve(
    path.dirname(fileURLToPath(import.meta.url)),
    "../../../../fixtures/golden/map-10.html",
  );

test.beforeEach(async ({ page }) => {
  await page.goto(pathToFileURL(artifact).href);
});

test("schema generation keeps adversarial inputs inside local budgets", () => {
  const schemas: ApiSchema[] = [
    {
      id: "array",
      kind: "array",
      items: "text",
      constraints: { min_items: 1_000_000 },
      confidence: "exact",
    },
    {
      id: "text",
      kind: "string",
      constraints: { pattern: "(a+)+$" },
      confidence: "exact",
    },
  ];
  const index = createSchemaIndex(schemas);
  const generated = generateValue("array", index, () => 0.5);

  expect(Array.isArray(generated)).toBeTruthy();
  expect(generated).toHaveLength(20);
  expect(validateValue("array", generated, index)).toContain(
    "$ has too few items",
  );
  expect(validateValue("text", "aaaa", index)).toContain(
    "$ uses a pattern outside the safe simulator subset",
  );

  const sequential = createSchemaIndex([
    {
      id: "sequential",
      kind: "string",
      constraints: { pattern: "^a*a*a*a*a*a*b$" },
      confidence: "exact",
    },
    {
      id: "fixed",
      kind: "string",
      constraints: { pattern: "^[a-z]{3}$" },
      confidence: "exact",
    },
  ]);
  expect(validateValue("sequential", "a".repeat(512), sequential)).toContain(
    "$ uses a pattern outside the safe simulator subset",
  );
  expect(validateValue("fixed", "abc", sequential)).toEqual([]);
});

test("intersection generation stays within the store object-key budget", () => {
  const schemas: ApiSchema[] = [
    { id: "text", kind: "string", confidence: "exact" },
    ...Array.from({ length: 20 }, (_, variant) => ({
      id: `variant-${variant}`,
      kind: "object" as const,
      confidence: "exact" as const,
      properties: Array.from({ length: 100 }, (_, property) => ({
        name: `field-${variant}-${property}`,
        schema_id: "text",
        required: true,
      })),
    })),
    {
      id: "intersection",
      kind: "intersection",
      variants: Array.from({ length: 20 }, (_, index) => `variant-${index}`),
      confidence: "exact",
    },
  ];

  const generated = generateValue(
    "intersection",
    createSchemaIndex(schemas),
    () => 0.5,
  );
  expect(generated).not.toBeNull();
  expect(Array.isArray(generated)).toBeFalsy();
  expect(Object.keys(generated as Record<string, JsonValue>)).toHaveLength(100);
});

test("schema validation rejects unchecked recursion and invalid formats", () => {
  const schemas = createSchemaIndex([
    {
      id: "node",
      kind: "object",
      properties: [{ name: "child", schema_id: "node", required: true }],
      confidence: "exact",
    },
    { id: "date", kind: "string", format: "date", confidence: "exact" },
    { id: "ipv4", kind: "string", format: "ipv4", confidence: "exact" },
    { id: "url", kind: "string", format: "url", confidence: "exact" },
  ] satisfies ApiSchema[]);
  let nested: JsonValue = {};
  for (let depth = 0; depth < 12; depth += 1) nested = { child: nested };

  expect(validateValue("node", nested, schemas).join("\n")).toContain(
    "exceeds the simulator validation depth",
  );
  expect(validateValue("date", "2026-02-30", schemas)).toContain(
    "$ must match the date format",
  );
  expect(validateValue("ipv4", "999.0.0.1", schemas)).toContain(
    "$ must match the ipv4 format",
  );
  expect(validateValue("url", "not a URL", schemas)).toContain(
    "$ must match the url format",
  );
});

test("virtual engine shares CRUD state across endpoint executions", () => {
  const apiMap = virtualApiMap();
  const planned = planVirtualBackend(apiMap, 42);
  const store = new VirtualBackendStore(planned);
  const engine = new VirtualBackendEngine(apiMap, store);

  expect(planned.operations["GET /orders"]?.kind).toBe("list");
  expect(planned.operations["PUT /orders/{id}"]?.kind).toBe("replace");
  expect(planned.operations["POST /orders"]?.resource).toBe("orders");
  const prefixedEndpoint = endpoint(
    "GET",
    "/api/v1/orders",
    undefined,
    "orders",
    "200",
  );
  const actionEndpoint = endpoint(
    "POST",
    "/api/v1/orders/{id}/cancel",
    undefined,
    "order",
    "200",
    true,
  );
  const routePlan = planVirtualBackend({
    ...apiMap,
    endpoints: [prefixedEndpoint, actionEndpoint],
  });
  expect(routePlan.operations[prefixedEndpoint.id]?.resource).toBe("orders");
  expect(routePlan.operations[actionEndpoint.id]).toMatchObject({
    resource: "orders",
    kind: "action",
    confidence: "inferred",
  });

  const firstList = engine.execute(
    "GET /orders",
    engine.defaultRequest("GET /orders"),
  );
  expect(firstList.status).toBe("200");
  expect(firstList.body).toHaveLength(2);

  const updateRequest = engine.defaultRequest("PUT /orders/{id}");
  updateRequest.body = { name: "Updated through PUT", role: "admin" };
  const update = engine.execute("PUT /orders/{id}", updateRequest);
  expect(update.status).toBe("200");
  expect(update.changedResource).toBe("orders");
  expect(update.body).toEqual({
    id: expect.any(String),
    name: "Updated through PUT",
  });
  expect(update.body).not.toHaveProperty("role");
  expect(store.resource("orders")?.records[0]).toHaveProperty("role", "admin");

  const updatedList = engine.execute(
    "GET /orders",
    engine.defaultRequest("GET /orders"),
  );
  expect(updatedList.body).toContainEqual(
    expect.objectContaining({ name: "Updated through PUT" }),
  );
  expect(updatedList.body).not.toContainEqual(
    expect.objectContaining({ role: "admin" }),
  );

  const create = engine.execute(
    "POST /orders",
    engine.defaultRequest("POST /orders"),
  );
  expect(create.status).toBe("201");
  expect(store.resource("orders")?.records).toHaveLength(3);

  const remove = engine.execute(
    "DELETE /orders/{id}",
    engine.defaultRequest("DELETE /orders/{id}"),
  );
  expect(remove.status).toBe("204");
  expect(store.resource("orders")?.records).toHaveLength(2);
});

test("planner separates nested resources and case-colliding names", () => {
  const nested = endpoint("GET", "/users/{userId}/orders", undefined, "orders", "200");
  const upper = endpoint("GET", "/Admin/Users", undefined, "orders", "200");
  const lower = endpoint("GET", "/admin/users", undefined, "orders", "200");
  const planned = planVirtualBackend({
    ...virtualApiMap(),
    endpoints: [nested, upper, lower],
  });

  expect(planned.operations[nested.id]).toMatchObject({
    kind: "list",
    primaryKey: "id",
  });
  const upperResource = planned.operations[upper.id]?.resource;
  const lowerResource = planned.operations[lower.id]?.resource;
  expect(upperResource).toBeDefined();
  expect(lowerResource).toBeDefined();
  expect(upperResource).not.toBe(lowerResource);
  expect(Object.keys(planned.resources)).toHaveLength(3);
});

test("virtual CRUD preserves valid current snapshots and unique primary keys", () => {
  for (let seed = 1; seed <= 32; seed += 1) {
    const apiMap = virtualApiMap();
    const store = new VirtualBackendStore(planVirtualBackend(apiMap, seed));
    const engine = new VirtualBackendEngine(apiMap, store);
    let state = seed;

    for (let step = 0; step < 80; step += 1) {
      state = (Math.imul(state, 1_664_525) + 1_013_904_223) >>> 0;
      const endpointId = [
        "GET /orders",
        "POST /orders",
        "PUT /orders/{id}",
        "DELETE /orders/{id}",
      ][state % 4]!;
      const beforeSnapshot = store.editableSnapshot();
      const request = engine.defaultRequest(endpointId);
      if (endpointId === "POST /orders" || endpointId === "PUT /orders/{id}") {
        request.body = { name: `seed-${seed}-step-${step}` };
      }
      const result = engine.execute(endpointId, request);

      if (!result.changedResource) {
        expect(store.editableSnapshot()).toEqual(beforeSnapshot);
      }
      expect(() => parseEditableSnapshot(store.editableSnapshot())).not.toThrow();
      const resource = store.resource("orders");
      expect(resource).toBeDefined();
      if (!resource) continue;
      const identities = resource.records.map((record) =>
        String(record[resource.primaryKey]),
      );
      expect(new Set(identities).size).toBe(identities.length);
    }
  }
});

test("virtual engine resolves declared error, wildcard, and pagination semantics", () => {
  const apiMap = virtualApiMap();
  apiMap.schemas?.push({
    id: "validation-error",
    kind: "object",
    confidence: "exact",
    properties: [
      { name: "message", schema_id: "name", required: true },
    ],
  });
  const create = apiMap.endpoints.find(
    (candidate) => candidate.id === "POST /orders",
  );
  if (!create?.contract) throw new Error("POST /orders contract is required");
  create.contract.responses ??= [];
  create.contract.responses.push({
    status: "400",
    contents: [
      { media_type: "application/json", schema_id: "validation-error" },
    ],
  });
  const wildcard = endpoint(
    "GET",
    "/archive",
    undefined,
    "orders",
    "2XX",
  );
  apiMap.endpoints.push(wildcard);

  const planned = planVirtualBackend(apiMap, 42);
  expect(planned.operations[wildcard.id]?.responseStatus).toBe("200");
  const store = new VirtualBackendStore(planned);
  const engine = new VirtualBackendEngine(apiMap, store);

  const invalid = engine.defaultRequest("POST /orders");
  invalid.body = null;
  const validation = engine.execute("POST /orders", invalid);
  expect(validation.status).toBe("400");
  expect(validation.responseErrors).toEqual([]);
  expect(validation.body).toEqual({ message: "Request validation failed" });

  const secondPage = engine.defaultRequest("GET /orders");
  secondPage.parameters.query = { page: 2, limit: 1 };
  const paged = engine.execute("GET /orders", secondPage);
  expect(paged.body).toHaveLength(1);
  expect(paged.body).toEqual([store.resource("orders")?.records[1]]);
});

test("editable backend rejects malformed or oversized resource shapes", () => {
  expect(() =>
    parseEditableSnapshot({ resources: {}, version: 1 }),
  ).toThrow("unsupported property version");
  expect(() =>
    parseEditableSnapshot({ resources: {}, revision: 1 }),
  ).toThrow("unsupported property revision");
  expect(() =>
    parseEditableSnapshot({ resources: [] }),
  ).toThrow("resources object");
  expect(() =>
    parseEditableSnapshot({
      resources: {
        orders: { primaryKey: "id", records: ["not-an-object"] },
      },
    }),
  ).toThrow("must be a JSON object");

  expect(() =>
    parseEditableSnapshot({ resources: {}, extra: true }),
  ).toThrow("unsupported property");
  expect(() =>
    parseEditableSnapshot({
      resources: { orders: { primaryKey: "", records: [] } },
    }),
  ).toThrow("primaryKey between 1 and 128 characters");
  expect(() =>
    parseEditableSnapshotText(`{
      "resources": {},
      "extra": true
    }`),
  ).toThrow("unsupported property");
  expect(() => parseEditableSnapshotText(" ".repeat(2_000_001))).toThrow(
    "2 MB",
  );
});

test("editable backend requires stable scalar primary keys", () => {
  expect(() =>
    parseEditableSnapshot({
      resources: {
        orders: { primaryKey: "id", records: [{ name: "missing" }] },
      },
    }),
  ).toThrow("must contain a string or number id");

  expect(() =>
    parseEditableSnapshot({
      resources: {
        orders: {
          primaryKey: "id",
          records: [{ id: "same" }, { id: "same" }],
        },
      },
    }),
  ).toThrow("duplicate id same");

  expect(() =>
    parseEditableSnapshot({
      resources: {
        orders: { primaryKey: "id", records: [{ id: { nested: true } }] },
      },
    }),
  ).toThrow("must contain a string or number id");
});

test("editable backend rejects empty resource names", () => {
  expect(() =>
    parseEditableSnapshot({
      resources: {
        "": { primaryKey: "id", records: [] },
      },
    }),
  ).toThrow("Resource names must contain between 1 and 128 characters");
});

test("virtual backend preserves prototype-like resource names safely", () => {
  const parsed = parseEditableSnapshotText(
    '{"resources":{"__proto__":{"primaryKey":"id","records":[]}}}',
  );
  expect(Object.hasOwn(parsed.resources, "__proto__")).toBeTruthy();
  expect(parsed.resources.__proto__).toEqual({ primaryKey: "id", records: [] });
  expect(Reflect.get(Object.prototype, "primaryKey")).toBeUndefined();

  const dangerousEndpoint = endpoint(
    "GET",
    "/__proto__",
    undefined,
    "entity",
    "200",
  );
  const planned = planVirtualBackend({
    ...virtualApiMap(),
    endpoints: [dangerousEndpoint],
  });
  expect(Object.hasOwn(planned.resources, "__proto__")).toBeTruthy();
  expect(planned.resources.__proto__?.records).toHaveLength(2);

  const store = new VirtualBackendStore(planned);
  expect(store.resource("constructor")).toBeUndefined();
  expect(store.operation("toString")).toBeUndefined();
});

test("schema fields build nested request bodies with typed values", () => {
  let body: JsonValue = {};
  body = updateBodyAtPointer(body, "/customer/name", "Ada");
  body = updateBodyAtPointer(
    body,
    "/customer/age",
    parseBodyFieldValue("37", "integer"),
  );
  body = updateBodyAtPointer(
    body,
    "/customer/active",
    parseBodyFieldValue("true", "boolean"),
  );

  expect(body).toEqual({
    customer: { name: "Ada", age: 37, active: true },
  });
  expect(() => parseBodyFieldValue("3.14", "integer")).toThrow(
    "Expected an integer",
  );

  const adversarialBody = updateBodyAtPointer(
    {},
    "/__proto__/apiSubwayPolluted",
    true,
  );
  expect(JSON.stringify(adversarialBody)).toBe(
    '{"__proto__":{"apiSubwayPolluted":true}}',
  );
  expect(Reflect.get(Object.prototype, "apiSubwayPolluted")).toBeUndefined();
});

test("response projection follows the endpoint schema and removes store-only fields", () => {
  const schemas: ApiSchema[] = [
    { id: "string", kind: "string", confidence: "exact" },
    { id: "uuid", kind: "string", format: "uuid", confidence: "exact" },
    {
      id: "response",
      kind: "object",
      confidence: "exact",
      properties: [
        { name: "id", schema_id: "uuid", required: true },
        { name: "name", schema_id: "string", required: true },
      ],
    },
  ];
  const projected = projectValueToSchema(
    "response",
    { name: "Visible", role: "store-only" },
    createSchemaIndex(schemas),
    () => 0.5,
  );

  expect(projected).toEqual({
    id: expect.stringMatching(
      /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    ),
    name: "Visible",
  });
  expect(projected).not.toHaveProperty("role");
});

test("search and method filters dim non-matching stations", async ({ page }) => {
  await page.locator("#search").fill("/orders");
  await expect(page.locator(".station.is-muted")).not.toHaveCount(0);
  await expect(page.locator("#result-count")).toContainText("stations");

  await page.locator("#method-filter").selectOption("GET");
  const visibleMethods = await page
    .locator(".station:not(.is-muted)")
    .evaluateAll((stations) =>
      stations.map((station) => (station as SVGElement).dataset.method),
    );
  expect(visibleMethods.every((method) => method === "GET")).toBeTruthy();
});

test("top-level controls expose stable accessible names", async ({ page }) => {
  await expect(
    page.getByRole("searchbox", { name: "Search endpoints", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Open virtual backend", exact: true }),
  ).toBeVisible();
  await expect(page.locator("footer")).toContainText("Virtual backend");
  await expect(page.locator("footer")).not.toContainText(/revision|version|schema v/i);

  const unlabeledControls = await page
    .locator("button, input, select, textarea, a")
    .evaluateAll((controls) =>
      controls
        .filter((control) => {
          const text = control.textContent?.trim() ?? "";
          const labels =
            control instanceof HTMLInputElement ||
            control instanceof HTMLSelectElement ||
            control instanceof HTMLTextAreaElement
              ? control.labels
              : null;
          return (
            !text &&
            !control.getAttribute("aria-label") &&
            !control.getAttribute("aria-labelledby") &&
            !labels?.length
          );
        })
        .map((control) => control.outerHTML.slice(0, 180)),
    );
  expect(unlabeledControls).toEqual([]);
});

test("stations open independent live request windows", async ({ page }) => {
  await openOrdersWindows(page);

  await expect(page.locator(".endpoint-workspace-window")).toHaveCount(2);
  const getWindow = page.locator(
    '.endpoint-workspace-window[data-method="GET"][data-path="/orders"]',
  );
  const putWindow = page.locator(
    '.endpoint-workspace-window[data-method="PUT"][data-path="/orders/{id}"]',
  );
  await expect(page.locator(".window-live-row")).toHaveCount(0);
  await expect(page.locator(".window-evidence")).toHaveCount(0);
  await expect(getWindow.locator(".response-schema-card")).toContainText(
    "Response · 200 · application/json",
  );
  await expect(
    getWindow.locator('[data-response-field-path="/0/name"]'),
  ).not.toHaveText("Run endpoint");
  const requestSchema = putWindow.locator(
    ".endpoint-request-section .body-schema-card",
  );
  await expect(requestSchema).toContainText(
    "Body · application/json",
  );
  await expect(requestSchema).toContainText(
    "Body JSON is built automatically",
  );
  await expect(
    putWindow.getByRole("button", { name: "Show request as schema" }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(
    putWindow.getByRole("button", { name: "Show request as JSON" }),
  ).toHaveAttribute("aria-pressed", "false");
  await expect(putWindow.locator('[data-body-field-path="/email"]')).toHaveAttribute(
    "type",
    "email",
  );
  await expect(putWindow.locator('[data-body-field-path="/role"]')).toHaveValue(
    '"admin"',
  );
  await expect(putWindow.locator(".response-schema-card")).toContainText(
    "200 · application/json",
  );
  await expect(putWindow.locator(".response-schema-card")).toContainText(
    "id *",
  );
  await expect(putWindow.locator(".response-schema-card")).toContainText(
    "Run endpoint",
  );
  await expect(
    putWindow.getByRole("button", { name: "Show response as schema" }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(
    putWindow.getByRole("button", { name: "Show response as JSON" }),
  ).toHaveAttribute("aria-pressed", "false");
  await expect(putWindow.getByRole("button", { name: "Run" })).toBeVisible();
});

test("request Schema and JSON views stay synchronized", async ({ page }) => {
  await openOrdersWindows(page);
  const putWindow = page.locator(
    '.endpoint-workspace-window[data-method="PUT"][data-path="/orders/{id}"]',
  );
  const schemaButton = putWindow.getByRole("button", {
    name: "Show request as schema",
  });
  const jsonButton = putWindow.getByRole("button", {
    name: "Show request as JSON",
  });

  await jsonButton.click();
  await expect(jsonButton).toHaveAttribute("aria-pressed", "true");
  await expect(putWindow.locator('[data-request-location="path"]')).toBeVisible();
  const bodyEditor = putWindow.locator("[data-request-body]");
  await expect(bodyEditor).toBeVisible();
  await bodyEditor.fill(
    JSON.stringify(
      {
        name: "Edited as JSON",
        email: "json@example.com",
        role: "member",
      },
      null,
      2,
    ),
  );

  await schemaButton.click();
  await expect(schemaButton).toHaveAttribute("aria-pressed", "true");
  await expect(putWindow.locator('[data-body-field-path="/name"]')).toHaveValue(
    "Edited as JSON",
  );
  await expect(putWindow.locator('[data-body-field-path="/email"]')).toHaveValue(
    "json@example.com",
  );
  await expect(putWindow.locator('[data-body-field-path="/role"]')).toHaveValue(
    '"member"',
  );

  await putWindow
    .locator('[data-body-field-path="/name"]')
    .fill("Edited as Schema");
  await jsonButton.click();
  expect(JSON.parse(await bodyEditor.inputValue())).toMatchObject({
    name: "Edited as Schema",
    email: "json@example.com",
    role: "member",
  });

  await bodyEditor.fill("{ invalid");
  await schemaButton.click();
  await expect(bodyEditor).toBeVisible();
  await expect(bodyEditor).toHaveAttribute("aria-invalid", "true");
  await expect(putWindow.locator(".request-view-error")).toBeVisible();
  await expect(jsonButton).toHaveAttribute("aria-pressed", "true");
});

test("PUT updates the current backend and an open GET updates live", async ({ page }) => {
  await openOrdersWindows(page);
  const getWindow = page.locator(
    '.endpoint-workspace-window[data-method="GET"][data-path="/orders"]',
  );
  const putWindow = page.locator(
    '.endpoint-workspace-window[data-method="PUT"][data-path="/orders/{id}"]',
  );

  await fillOrderSchemaFields(
    putWindow,
    "Updated from PUT",
    "updated@example.com",
    "member",
  );
  await putWindow
    .getByRole("button", { name: "Show request as JSON" })
    .click();
  expect(JSON.parse(await putWindow.locator("[data-request-body]").inputValue())).toMatchObject({
    name: "Updated from PUT",
    email: "updated@example.com",
    role: "member",
  });
  await putWindow.getByRole("button", { name: "Run", exact: true }).click();

  await expect(
    getWindow.locator('[data-response-field-path="/0/name"]'),
  ).toContainText(
    "Updated from PUT",
  );
  await expect(getWindow.locator(".response-schema-card")).not.toContainText(
    "role",
  );
  await expect(getWindow.locator(".window-live-notice")).toContainText(
    "PUT /orders/{id} changed orders",
  );
  await expect(putWindow.locator(".response-status")).toContainText("200 OK");
  await expect(
    putWindow.locator('[data-response-field-path="/name"]'),
  ).toContainText("Updated from PUT");
  await expect(putWindow.locator(".response-schema-card")).not.toContainText(
    "role",
  );

  await putWindow
    .getByRole("button", { name: "Show response as JSON" })
    .click();
  await expect(putWindow.locator(".response-code")).toContainText(
    "Updated from PUT",
  );
  await expect(putWindow.locator(".response-code")).not.toContainText('"role"');
  await expect(
    putWindow.getByRole("button", { name: "Show response as JSON" }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(getWindow.locator(".response-schema-card")).toBeVisible();
  await expect(getWindow.locator(".response-code")).toHaveCount(0);
});

test("applying backend JSON resets endpoint windows against the replacement store", async ({
  page,
}) => {
  await openOrdersWindows(page);
  const putWindow = page.locator(
    '.endpoint-workspace-window[data-method="PUT"][data-path="/orders/{id}"]',
  );
  await fillOrderSchemaFields(
    putWindow,
    "Pending request",
    "pending@example.com",
    "member",
  );
  await page.locator("#open-backend").click();
  const editor = page.locator("[data-backend-json]");
  const snapshot = parseEditableSnapshot(JSON.parse(await editor.inputValue()));
  snapshot.resources.orders!.records[0]!.name = "Edited in backend JSON";
  await editor.fill(JSON.stringify(snapshot, null, 2));
  await page.getByRole("button", { name: "Apply JSON", exact: true }).click();

  const getWindow = page.locator(
    '.endpoint-workspace-window[data-method="GET"][data-path="/orders"]',
  );
  await expect(
    getWindow.locator('[data-response-field-path="/0/name"]'),
  ).toContainText(
    "Edited in backend JSON",
  );
  await expect(
    putWindow.locator('[data-body-field-path="/name"]'),
  ).not.toHaveValue("Pending request");
  await expect(putWindow.locator(".response-status")).toHaveCount(0);
  await expect(
    putWindow.getByRole("button", { name: "Show request as schema" }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(putWindow.locator(".window-live-notice")).toContainText(
    "Backend JSON applied · window reset",
  );
  await expect(page.locator(".backend-dirty-state")).toHaveText("Synced");
});

test("backend import remains local until apply and export preserves applied state", async ({
  page,
}) => {
  await page.locator('.station[data-method="GET"][data-path="/orders"]').click();
  await page.locator("#open-backend").click();
  const backendWindow = page.locator(".backend-workspace-window");
  const editor = backendWindow.locator("[data-backend-json]");
  const imported = parseEditableSnapshot(JSON.parse(await editor.inputValue()));
  imported.resources.orders!.records[0]!.name = "Imported but unpublished";

  await backendWindow.locator("[data-backend-file]").setInputFiles({
    name: "backend-fixture.json",
    mimeType: "application/json",
    buffer: Buffer.from(JSON.stringify(imported, null, 2)),
  });

  await expect(backendWindow.locator(".window-live-notice")).toContainText(
    "loaded · Apply JSON to replace the backend",
  );
  await expect(backendWindow.locator(".backend-dirty-state")).toHaveText(
    "Unsaved changes",
  );
  const getWindow = page.locator(
    '.endpoint-workspace-window[data-method="GET"][data-path="/orders"]',
  );
  await expect(getWindow).not.toContainText("Imported but unpublished");

  await backendWindow
    .getByRole("button", { name: "Apply JSON", exact: true })
    .click();
  await expect(
    getWindow.locator('[data-response-field-path="/0/name"]'),
  ).toContainText("Imported but unpublished");

  const downloadPromise = page.waitForEvent("download");
  await backendWindow.getByRole("button", { name: "Export" }).click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toBe("api-subway-backend.json");
  const downloadPath = await download.path();
  expect(downloadPath).not.toBeNull();
  if (!downloadPath) return;
  const exported = parseEditableSnapshotText(
    await readFile(downloadPath, "utf8"),
  );
  expect(Object.keys(exported)).toEqual(["resources"]);
  expect(exported.resources.orders?.records[0]?.name).toBe(
    "Imported but unpublished",
  );
});

test("invalid backend JSON stays local and does not replace the backend", async ({
  page,
}) => {
  await page.locator("#open-backend").click();
  await page.locator("[data-backend-json]").fill("{ invalid");
  await page.getByRole("button", { name: "Apply JSON", exact: true }).click();

  await expect(page.locator(".backend-editor-error")).toBeVisible();
  await expect(page.locator(".backend-dirty-state")).toHaveText(
    "Unsaved changes",
  );
});

test("reset restores generated state and notifies live windows", async ({ page }) => {
  await openOrdersWindows(page);
  const getWindow = page.locator(
    '.endpoint-workspace-window[data-method="GET"][data-path="/orders"]',
  );
  const putWindow = page.locator(
    '.endpoint-workspace-window[data-method="PUT"][data-path="/orders/{id}"]',
  );
  await fillOrderSchemaFields(
    putWindow,
    "Temporary",
    "temp@example.com",
    "member",
  );
  await putWindow.getByRole("button", { name: "Run", exact: true }).click();
  await expect(getWindow).toContainText("Temporary");

  await page.locator("#reset-backend").click();
  await expect(getWindow).not.toContainText("Temporary");
  await expect(getWindow.locator(".window-live-notice")).toContainText(
    "Backend reset",
  );
  await expect(
    putWindow.locator('[data-body-field-path="/name"]'),
  ).not.toHaveValue("Temporary");
  await expect(putWindow.locator(".response-status")).toHaveCount(0);
  await expect(putWindow.locator(".window-live-notice")).toContainText(
    "Backend reset · window reset",
  );
});

test("desktop endpoint windows grow with their full content", async ({ page }) => {
  await page.locator('.station[data-method="GET"][data-path="/orders"]').click();
  const workspaceWindow = page.locator(
    '.endpoint-workspace-window[data-method="GET"][data-path="/orders"]',
  );
  const before = await workspaceWindow.boundingBox();
  expect(before).not.toBeNull();
  if (!before) return;

  await workspaceWindow.getByRole("button", { name: "Run", exact: true }).click();
  await expect(workspaceWindow.locator(".window-live-notice")).toContainText(
    "Request completed",
  );
  const after = await workspaceWindow.boundingBox();
  expect(after).not.toBeNull();
  if (!after) return;
  expect(after.height).toBeGreaterThan(before.height);

  const windowLayout = await workspaceWindow.evaluate((element) => {
    const content = element.querySelector<HTMLElement>(
      ".workspace-window-content",
    );
    const style = getComputedStyle(element);
    const contentStyle = content ? getComputedStyle(content) : undefined;
    return {
      maxHeight: style.maxHeight,
      resize: style.resize,
      contentOverflowY: contentStyle?.overflowY,
      contentClientHeight: content?.clientHeight ?? 0,
      contentScrollHeight: content?.scrollHeight ?? 0,
    };
  });
  expect(windowLayout).toMatchObject({
    maxHeight: "none",
    resize: "horizontal",
    contentOverflowY: "visible",
  });
  expect(windowLayout.contentScrollHeight).toBeLessThanOrEqual(
    windowLayout.contentClientHeight + 1,
  );

  const mapPanel = page.locator(".map-panel");
  const panelMetrics = await mapPanel.evaluate((element) => ({
    clientHeight: element.clientHeight,
    scrollHeight: element.scrollHeight,
  }));
  expect(panelMetrics.scrollHeight).toBeGreaterThan(panelMetrics.clientHeight);
  await mapPanel.evaluate((element) => element.scrollTo(0, element.scrollHeight));
  await expect(workspaceWindow.locator(".window-live-notice")).toBeInViewport();
});

test("desktop endpoint windows can be dragged independently", async ({ page }) => {
  await page.locator('.station[data-method="GET"][data-path="/orders"]').click();
  const workspaceWindow = page.locator(
    '.endpoint-workspace-window[data-method="GET"][data-path="/orders"]',
  );
  const titlebar = workspaceWindow.locator("[data-window-drag-handle]");
  const before = await workspaceWindow.boundingBox();
  const handle = await titlebar.boundingBox();
  expect(before).not.toBeNull();
  expect(handle).not.toBeNull();
  if (!before || !handle) return;

  await page.mouse.move(handle.x + 80, handle.y + 20);
  await page.mouse.down();
  await page.mouse.move(handle.x + 170, handle.y + 80, { steps: 4 });
  await page.mouse.up();

  const after = await workspaceWindow.boundingBox();
  expect(after?.x).toBeGreaterThan(before.x + 60);
  expect(after?.y).toBeGreaterThan(before.y + 35);
});

test("line filters, theme, and zoom controls update the map", async ({ page }) => {
  await page.locator('#kind-filters [data-kind="datastore"]').click();
  await expect(page.locator(".dependency-line.is-muted")).not.toHaveCount(0);

  await page.locator("#theme-toggle").click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "paper");

  const before = await page.locator(".api-map").getAttribute("viewBox");
  await page.locator("#zoom-in").click();
  await expect(page.locator(".api-map")).not.toHaveAttribute(
    "viewBox",
    before ?? "",
  );
  await page.locator("#fit-map").click();
  await expect(page.locator(".api-map")).toHaveAttribute(
    "viewBox",
    before ?? "",
  );
});

test("loads and executes without browser console errors", async ({ page }) => {
  const errors: string[] = [];
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  page.on("pageerror", (error) => errors.push(error.message));

  await page.reload();
  await openOrdersWindows(page);
  await page
    .locator(
      '.endpoint-workspace-window[data-method="PUT"][data-path="/orders/{id}"] [data-window-action="run"]',
    )
    .click();
  expect(errors).toEqual([]);
});

test("mobile uses a live workspace sheet with tabs for every open station", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.reload();
  await openOrdersWindows(page);

  const getWindow = page.locator(
    '.endpoint-workspace-window[data-method="GET"][data-path="/orders"]',
  );
  const putWindow = page.locator(
    '.endpoint-workspace-window[data-method="PUT"][data-path="/orders/{id}"]',
  );
  await expect(page.locator("#map-viewport")).toBeVisible();
  await expect(putWindow).toBeVisible();
  await expect(getWindow).toBeHidden();
  const mobileDock = page.locator(".workspace-mobile-dock");
  await expect(mobileDock).toHaveCount(1);
  await expect(mobileDock).toBeVisible();
  await expect(putWindow.locator(".request-view-switch")).toBeVisible();
  await putWindow
    .getByRole("button", { name: "Show request as JSON" })
    .click();
  await expect(putWindow.locator("[data-request-body]")).toBeVisible();
  await expect(putWindow.locator(".response-view-switch")).toBeVisible();
  await putWindow
    .getByRole("button", { name: "Show response as JSON" })
    .click();
  await expect(putWindow.locator(".response-code")).toBeVisible();

  await mobileDock.locator("button").first().click();
  await expect(getWindow).toBeVisible();
  await expect(putWindow).toBeHidden();

  const viewportMetrics = await page.evaluate(() => ({
    clientWidth: document.body.clientWidth,
    scrollWidth: document.body.scrollWidth,
    viewportWidth: window.innerWidth,
  }));
  expect(viewportMetrics).toEqual({
    clientWidth: 390,
    scrollWidth: 390,
    viewportWidth: 390,
  });
});

const openOrdersWindows = async (page: Page): Promise<void> => {
  await page.locator('.station[data-method="GET"][data-path="/orders"]').click();
  if ((page.viewportSize()?.width ?? 1_440) <= 900) {
    await page.getByRole("button", { name: "Minimize GET /orders" }).click();
  }
  await page.locator('.station[data-method="PUT"][data-path="/orders/{id}"]').click();
};

const fillOrderSchemaFields = async (
  window: ReturnType<Page["locator"]>,
  name: string,
  email: string,
  role: "admin" | "member",
): Promise<void> => {
  await window.locator('[data-body-field-path="/name"]').fill(name);
  await window.locator('[data-body-field-path="/email"]').fill(email);
  await window
    .locator('[data-body-field-path="/role"]')
    .selectOption(JSON.stringify(role));
};

const virtualApiMap = (): ApiMap => {
  const schemas: ApiSchema[] = [
    { id: "id", kind: "string", format: "uuid", confidence: "exact" },
    { id: "name", kind: "string", confidence: "exact" },
    {
      id: "order-input",
      kind: "object",
      confidence: "exact",
      properties: [{ name: "name", schema_id: "name", required: true }],
    },
    {
      id: "order",
      kind: "object",
      confidence: "exact",
      properties: [
        { name: "id", schema_id: "id", required: true },
        { name: "name", schema_id: "name", required: true },
      ],
    },
    {
      id: "orders",
      kind: "array",
      confidence: "exact",
      items: "order",
    },
  ];
  const endpoints: Endpoint[] = [
    endpoint("GET", "/orders", undefined, "orders", "200"),
    endpoint("POST", "/orders", "order-input", "order", "201"),
    endpoint("PUT", "/orders/{id}", "order-input", "order", "200", true),
    endpoint("DELETE", "/orders/{id}", undefined, undefined, "204", true),
  ];
  return {
    endpoints,
    schemas,
    dependencies: [],
    relations: [],
    diagnostics: [],
  };
};

const endpoint = (
  method: string,
  routePath: string,
  bodySchema: string | undefined,
  responseSchema: string | undefined,
  status: string,
  itemRoute = false,
): Endpoint => ({
  id: `${method} ${routePath}`,
  method,
  path: routePath,
  display_path: routePath,
  framework: "openapi",
  contract: {
    confidence: "exact",
    request: {
      parameters: itemRoute
        ? [
            {
              name: "id",
              location: "path",
              required: true,
              schema_id: "id",
            },
          ]
        : [],
      bodies: bodySchema
        ? [
            {
              media_type: "application/json",
              schema_id: bodySchema,
              required: true,
            },
          ]
        : [],
    },
    responses: [
      {
        status,
        contents: responseSchema
          ? [{ media_type: "application/json", schema_id: responseSchema }]
          : [],
      },
    ],
  },
});
