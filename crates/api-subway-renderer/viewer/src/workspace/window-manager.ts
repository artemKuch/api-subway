import { createSchemaIndex, type JsonValue } from "../schema-simulator";
import type {
  ApiMap,
  ApiSchema,
  Endpoint,
  ParameterLocation,
} from "../types";
import { VirtualBackendEngine } from "../virtual-backend/engine";
import { setDictionaryValue } from "../virtual-backend/dictionary";
import {
  MAX_BACKEND_JSON_BYTES,
  parseEditableSnapshotText,
  VirtualBackendStore,
} from "../virtual-backend/store";
import type {
  BackendChange,
  ExecutionResult,
  VirtualRequest,
} from "../virtual-backend/types";
import { renderBackendWindow } from "./backend-window";
import { renderEndpointWindow } from "./endpoint-window";
import { escapeHtml, methodClass, prettyJson } from "./html";
import {
  parseBodyFieldValue,
  updateBodyAtPointer,
  type BodyFieldKind,
} from "./schema-fields";
import type {
  BackendWorkspaceWindow,
  EndpointWorkspaceWindow,
  PayloadViewMode,
  WindowAnchor,
  WorkspaceWindow,
} from "./types";

interface DragState {
  pointerId: number;
  windowId: string;
  startX: number;
  startY: number;
  originX: number;
  originY: number;
  element: HTMLElement;
}

const backendWindowId = "virtual-backend";
const parameterLocations: ParameterLocation[] = [
  "path",
  "query",
  "header",
  "cookie",
];
const MAX_JSON_INPUT_NODES = 100_000;

export class WorkspaceWindowManager {
  private readonly endpointById: Map<string, Endpoint>;
  private readonly schemas: Map<string, ApiSchema>;
  private readonly windows = new Map<string, WorkspaceWindow>();
  private readonly compactLayout = globalThis.matchMedia("(max-width: 900px)");
  private activeWindowId?: string;
  private drag?: DragState;
  private zIndex = 10;
  private cascade = 0;
  private committing = false;
  private queuedChange?: BackendChange;

  constructor(
    private readonly root: HTMLElement,
    map: ApiMap,
    private readonly store: VirtualBackendStore,
    private readonly engine: VirtualBackendEngine,
  ) {
    this.endpointById = new Map(
      map.endpoints.map((endpoint) => [endpoint.id, endpoint]),
    );
    this.schemas = createSchemaIndex(map.schemas);
    this.root.addEventListener("click", (event) => this.handleClick(event));
    this.root.addEventListener("input", (event) => this.handleInput(event));
    this.root.addEventListener("change", (event) => {
      void this.handleChange(event);
    });
    this.root.addEventListener("pointerdown", (event) =>
      this.handlePointerDown(event),
    );
    this.root.addEventListener("pointermove", (event) =>
      this.handlePointerMove(event),
    );
    this.root.addEventListener("pointerup", (event) =>
      this.handlePointerUp(event),
    );
    this.root.addEventListener("pointercancel", (event) =>
      this.handlePointerUp(event),
    );
    this.store.subscribe((change) => this.receiveBackendChange(change));
  }

  openEndpoint(endpointId: string, anchor?: WindowAnchor): void {
    const existing = this.windows.get(endpointWindowId(endpointId));
    if (existing?.kind === "endpoint") {
      existing.minimized = false;
      this.focusWindow(existing.id);
      this.render();
      return;
    }
    const endpoint = this.endpointById.get(endpointId);
    if (!endpoint) return;
    const request = this.engine.defaultRequest(endpointId);
    const geometry = this.endpointGeometry(anchor);
    const workspaceWindow: EndpointWorkspaceWindow = {
      id: endpointWindowId(endpointId),
      kind: "endpoint",
      endpointId,
      request,
      bodyText: prettyJson(request.body),
      requestView: "schema",
      responseView: "schema",
      minimized: false,
      ...geometry,
    };
    if (isWatchable(endpoint.method) || endpoint.method === "OPTIONS") {
      workspaceWindow.response = this.engine.execute(endpointId, request);
    }
    this.windows.set(workspaceWindow.id, workspaceWindow);
    this.activeWindowId = workspaceWindow.id;
    this.render();
  }

  openBackend(): void {
    const existing = this.windows.get(backendWindowId);
    if (existing?.kind === "backend") {
      existing.minimized = false;
      this.focusWindow(existing.id);
      this.render();
      return;
    }
    const workspaceWindow: BackendWorkspaceWindow = {
      id: backendWindowId,
      kind: "backend",
      jsonText: prettyJson(this.store.editableSnapshot()),
      dirty: false,
      minimized: false,
      ...this.backendGeometry(),
    };
    this.windows.set(workspaceWindow.id, workspaceWindow);
    this.activeWindowId = workspaceWindow.id;
    this.render();
  }

  resetBackend(): void {
    this.commit(() => this.store.reset());
    const backend = this.windows.get(backendWindowId);
    if (backend?.kind === "backend") {
      backend.dirty = false;
      backend.error = undefined;
      backend.jsonText = prettyJson(this.store.editableSnapshot());
    }
    this.flushQueuedChange();
  }

  private render(): void {
    this.captureGeometry();
    const sortedWindows = [...this.windows.values()].sort(
      (left, right) => left.zIndex - right.zIndex,
    );
    this.root.innerHTML =
      this.renderMobileDock(sortedWindows) +
      sortedWindows
        .map((workspaceWindow) => {
          const active = workspaceWindow.id === this.activeWindowId;
          if (workspaceWindow.kind === "backend") {
            return renderBackendWindow(workspaceWindow, active);
          }
          const endpoint = this.endpointById.get(workspaceWindow.endpointId);
          if (!endpoint) return "";
          return renderEndpointWindow(
            workspaceWindow,
            endpoint,
            this.engine.operation(endpoint.id),
            this.schemas,
            active,
          );
        })
        .join("");
    this.applyActiveState();
  }

  private renderMobileDock(windows: WorkspaceWindow[]): string {
    if (windows.length < 2) return "";
    return `<nav class="workspace-mobile-dock" aria-label="Open API windows">${windows
      .map((workspaceWindow) => {
        const active = workspaceWindow.id === this.activeWindowId;
        if (workspaceWindow.kind === "backend") {
          return `<button type="button" data-focus-window="${backendWindowId}" class="${active ? "active" : ""}"><span class="dock-method backend">DB</span><span>Backend</span></button>`;
        }
        const endpoint = this.endpointById.get(workspaceWindow.endpointId);
        return endpoint
          ? `<button type="button" data-focus-window="${escapeHtml(workspaceWindow.id)}" class="${active ? "active" : ""}"><span class="dock-method ${methodClass(endpoint.method)}">${escapeHtml(endpoint.method)}</span><span>${escapeHtml(endpoint.path)}</span></button>`
          : "";
      })
      .join("")}</nav>`;
  }

  private handleClick(event: MouseEvent): void {
    if (!(event.target instanceof Element)) return;
    const focusButton = event.target.closest<HTMLElement>("[data-focus-window]");
    if (focusButton?.dataset.focusWindow) {
      const targetWindow = this.windows.get(focusButton.dataset.focusWindow);
      if (targetWindow?.minimized) {
        targetWindow.minimized = false;
        this.focusWindow(targetWindow.id);
        this.render();
      } else {
        this.focusWindow(focusButton.dataset.focusWindow);
      }
      return;
    }
    const article = event.target.closest<HTMLElement>("[data-window-id]");
    if (!article?.dataset.windowId) return;
    this.focusWindow(article.dataset.windowId);
    const actionButton = event.target.closest<HTMLButtonElement>(
      "[data-window-action]",
    );
    if (!actionButton?.dataset.windowAction) return;
    const workspaceWindow = this.windows.get(article.dataset.windowId);
    if (!workspaceWindow) return;
    switch (actionButton.dataset.windowAction) {
      case "close":
        this.closeWindow(workspaceWindow.id);
        break;
      case "minimize":
        workspaceWindow.minimized = !workspaceWindow.minimized;
        this.render();
        break;
      case "run":
        if (workspaceWindow.kind === "endpoint") {
          this.runEndpoint(workspaceWindow);
        }
        break;
      case "request-schema":
      case "request-json":
        if (workspaceWindow.kind === "endpoint") {
          this.switchRequestView(
            workspaceWindow,
            actionButton.dataset.windowAction === "request-schema"
              ? "schema"
              : "json",
          );
        }
        break;
      case "response-schema":
      case "response-json":
        if (workspaceWindow.kind === "endpoint") {
          workspaceWindow.responseView =
            actionButton.dataset.windowAction === "response-schema"
              ? "schema"
              : "json";
          this.render();
        }
        break;
      case "backend-apply":
        if (workspaceWindow.kind === "backend") {
          this.applyBackendJson(workspaceWindow);
        }
        break;
      case "backend-reset":
        this.resetBackend();
        break;
      case "backend-import":
        article.querySelector<HTMLInputElement>("[data-backend-file]")?.click();
        break;
      case "backend-export":
        this.exportBackend();
        break;
    }
  }

  private handleInput(event: Event): void {
    if (
      !(
        event.target instanceof HTMLInputElement ||
        event.target instanceof HTMLTextAreaElement ||
        event.target instanceof HTMLSelectElement
      )
    ) {
      return;
    }
    const article = event.target.closest<HTMLElement>("[data-window-id]");
    if (!article?.dataset.windowId) return;
    const workspaceWindow = this.windows.get(article.dataset.windowId);
    if (!workspaceWindow) return;
    if (workspaceWindow.kind === "backend" && event.target.matches("[data-backend-json]")) {
      workspaceWindow.jsonText = event.target.value;
      workspaceWindow.dirty = true;
      workspaceWindow.error = undefined;
      article.querySelector(".backend-dirty-state")?.classList.add("dirty");
      const dirtyState = article.querySelector(".backend-dirty-state");
      if (dirtyState) dirtyState.textContent = "Unsaved changes";
      return;
    }
    if (workspaceWindow.kind !== "endpoint") return;
    const bodyFieldPath = event.target.dataset.bodyFieldPath;
    const bodyFieldKind = event.target.dataset.bodyFieldKind as
      | BodyFieldKind
      | undefined;
    if (bodyFieldPath !== undefined && bodyFieldKind) {
      const required = event.target.dataset.bodyFieldRequired === "true";
      try {
        const value =
          !required && event.target.value.trim() === ""
            ? undefined
            : parseBodyFieldValue(event.target.value, bodyFieldKind);
        workspaceWindow.request.body = updateBodyAtPointer(
          workspaceWindow.request.body,
          bodyFieldPath,
          value,
        );
        workspaceWindow.bodyText = prettyJson(workspaceWindow.request.body);
        const bodyEditor = article.querySelector<HTMLTextAreaElement>(
          "[data-request-body]",
        );
        if (bodyEditor) bodyEditor.value = workspaceWindow.bodyText;
        event.target.classList.remove("invalid");
        event.target.removeAttribute("aria-invalid");
        event.target.removeAttribute("title");
      } catch (error) {
        event.target.classList.add("invalid");
        event.target.setAttribute("aria-invalid", "true");
        event.target.title = errorMessage(error, "Invalid field value");
      }
      return;
    }
    if (event.target.matches("[data-request-body]")) {
      workspaceWindow.bodyText = event.target.value;
      workspaceWindow.requestError = undefined;
      event.target.removeAttribute("aria-invalid");
      article.querySelector(".request-view-error")?.remove();
      return;
    }
    const location = event.target.dataset.requestLocation as
      | ParameterLocation
      | undefined;
    const name = event.target.dataset.requestName;
    if (!location || !name || !parameterLocations.includes(location)) return;
    const parsed = parseJsonField(event.target.value);
    if (parsed === undefined) {
      delete workspaceWindow.request.parameters[location][name];
    } else {
      setDictionaryValue(
        workspaceWindow.request.parameters[location],
        name,
        parsed,
      );
    }
  }

  private async handleChange(event: Event): Promise<void> {
    if (!(event.target instanceof HTMLInputElement)) return;
    if (!event.target.matches("[data-backend-file]")) return;
    const article = event.target.closest<HTMLElement>("[data-window-id]");
    const workspaceWindow = article?.dataset.windowId
      ? this.windows.get(article.dataset.windowId)
      : undefined;
    const file = event.target.files?.[0];
    event.target.value = "";
    if (workspaceWindow?.kind !== "backend" || !file) return;
    if (file.size > MAX_BACKEND_JSON_BYTES) {
      workspaceWindow.error = "Import is limited to 2 MB";
      this.render();
      return;
    }
    try {
      const text = await file.text();
      parseEditableSnapshotText(text);
      workspaceWindow.jsonText = text;
      workspaceWindow.dirty = true;
      workspaceWindow.error = undefined;
      workspaceWindow.notice = `${file.name} loaded · Apply JSON to replace the backend`;
    } catch (error) {
      workspaceWindow.error = errorMessage(error, "The selected file is not valid JSON");
    }
    this.render();
  }

  private runEndpoint(workspaceWindow: EndpointWorkspaceWindow): void {
    let body: JsonValue;
    try {
      body = parseJson(workspaceWindow.bodyText);
    } catch (error) {
      const message = errorMessage(error, "Request body is not valid JSON");
      workspaceWindow.requestError = message;
      workspaceWindow.response = inputError(
        workspaceWindow.endpointId,
        message,
      );
      workspaceWindow.notice = "Request was not sent to the virtual backend";
      this.render();
      return;
    }
    workspaceWindow.requestError = undefined;
    workspaceWindow.request.body = body;
    try {
      this.committing = true;
      workspaceWindow.response = this.engine.execute(
        workspaceWindow.endpointId,
        workspaceWindow.request,
      );
      workspaceWindow.notice = workspaceWindow.response.changedResource
        ? `Updated ${workspaceWindow.response.changedResource} in virtual backend`
        : "Request completed";
    } catch (error) {
      workspaceWindow.response = inputError(
        workspaceWindow.endpointId,
        errorMessage(error, "Virtual endpoint execution failed"),
      );
    } finally {
      this.committing = false;
    }
    this.flushQueuedChange();
  }

  private switchRequestView(
    workspaceWindow: EndpointWorkspaceWindow,
    view: PayloadViewMode,
  ): void {
    if (view === "json") {
      workspaceWindow.requestView = "json";
      this.render();
      return;
    }
    try {
      workspaceWindow.request.body = parseJson(workspaceWindow.bodyText);
      workspaceWindow.requestError = undefined;
      workspaceWindow.requestView = "schema";
    } catch (error) {
      workspaceWindow.requestError = errorMessage(
        error,
        "Request body is not valid JSON",
      );
    }
    this.render();
  }

  private applyBackendJson(workspaceWindow: BackendWorkspaceWindow): void {
    try {
      const parsed = parseEditableSnapshotText(workspaceWindow.jsonText);
      this.committing = true;
      this.store.replaceEditableSnapshot(parsed);
      workspaceWindow.dirty = false;
      workspaceWindow.error = undefined;
      workspaceWindow.notice = "Backend JSON applied";
      workspaceWindow.jsonText = prettyJson(this.store.editableSnapshot());
    } catch (error) {
      workspaceWindow.error = errorMessage(error, "Virtual backend JSON is invalid");
      workspaceWindow.notice = undefined;
    } finally {
      this.committing = false;
    }
    this.flushQueuedChange();
  }

  private exportBackend(): void {
    const blob = new Blob([prettyJson(this.store.editableSnapshot())], {
      type: "application/json",
    });
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.download = "api-subway-backend.json";
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 0);
  }

  private receiveBackendChange(change: BackendChange): void {
    if (this.committing) {
      this.queuedChange = change;
      return;
    }
    this.applyBackendChange(change);
  }

  private flushQueuedChange(): void {
    const change = this.queuedChange;
    this.queuedChange = undefined;
    if (change) {
      this.applyBackendChange(change);
    } else {
      this.render();
    }
  }

  private applyBackendChange(change: BackendChange): void {
    const replacesBackend = change.resource === "*";
    const source = this.endpointById.get(change.endpointId);
    const sourceLabel = source
      ? `${source.method} ${source.path}`
      : change.endpointId === "virtual-backend:reset"
        ? "Backend reset"
        : "Backend JSON";
    const replacementLabel =
      change.endpointId === "virtual-backend:reset"
        ? "Backend reset"
        : "Backend JSON applied";
    for (const workspaceWindow of this.windows.values()) {
      if (workspaceWindow.kind === "backend") {
        if (!workspaceWindow.dirty) {
          workspaceWindow.jsonText = prettyJson(this.store.editableSnapshot());
        }
        workspaceWindow.notice = replacesBackend
          ? `${replacementLabel} · endpoint windows reset`
          : `${sourceLabel} updated ${change.resource}`;
        continue;
      }
      if (replacesBackend) {
        this.resetEndpointWindow(workspaceWindow, replacementLabel);
        continue;
      }
      const operation = this.engine.operation(workspaceWindow.endpointId);
      const affected = operation.resource === change.resource;
      if (!affected) continue;
      workspaceWindow.notice = `${sourceLabel} changed ${operation.resource}`;
      const endpoint = this.endpointById.get(workspaceWindow.endpointId);
      if (endpoint && isWatchable(endpoint.method)) {
        workspaceWindow.response = this.engine.execute(
          workspaceWindow.endpointId,
          workspaceWindow.request,
        );
      }
    }
    this.render();
  }

  private resetEndpointWindow(
    workspaceWindow: EndpointWorkspaceWindow,
    sourceLabel: string,
  ): void {
    const endpoint = this.endpointById.get(workspaceWindow.endpointId);
    const request = this.engine.defaultRequest(workspaceWindow.endpointId);
    workspaceWindow.request = request;
    workspaceWindow.bodyText = prettyJson(request.body);
    workspaceWindow.requestView = "schema";
    workspaceWindow.responseView = "schema";
    workspaceWindow.requestError = undefined;
    workspaceWindow.notice = `${sourceLabel} · window reset`;
    workspaceWindow.response =
      endpoint && (isWatchable(endpoint.method) || endpoint.method === "OPTIONS")
        ? this.engine.execute(workspaceWindow.endpointId, request)
        : undefined;
  }

  private commit(callback: () => BackendChange): void {
    try {
      this.committing = true;
      callback();
    } finally {
      this.committing = false;
    }
  }

  private closeWindow(windowId: string): void {
    this.windows.delete(windowId);
    if (this.activeWindowId === windowId) {
      this.activeWindowId = [...this.windows.values()].sort(
        (left, right) => right.zIndex - left.zIndex,
      )[0]?.id;
    }
    this.render();
  }

  private focusWindow(windowId: string): void {
    const workspaceWindow = this.windows.get(windowId);
    if (!workspaceWindow) return;
    this.zIndex += 1;
    workspaceWindow.zIndex = this.zIndex;
    this.activeWindowId = windowId;
    this.applyActiveState();
  }

  private applyActiveState(): void {
    this.root.querySelectorAll<HTMLElement>("[data-window-id]").forEach((element) => {
      element.classList.toggle(
        "active",
        element.dataset.windowId === this.activeWindowId,
      );
      element.style.zIndex = String(
        this.windows.get(element.dataset.windowId ?? "")?.zIndex ?? 1,
      );
    });
    this.root.querySelectorAll<HTMLElement>("[data-focus-window]").forEach((element) => {
      element.classList.toggle(
        "active",
        element.dataset.focusWindow === this.activeWindowId,
      );
    });
    const openEndpoints = new Set(
      [...this.windows.values()]
        .filter(
          (workspaceWindow): workspaceWindow is EndpointWorkspaceWindow =>
            workspaceWindow.kind === "endpoint",
        )
        .map((workspaceWindow) => workspaceWindow.endpointId),
    );
    document.querySelectorAll<SVGGElement>(".station").forEach((station) => {
      const endpointId = station.dataset.endpointId;
      station.classList.toggle(
        "window-open",
        Boolean(endpointId && openEndpoints.has(endpointId)),
      );
      station.classList.toggle(
        "window-active",
        Boolean(
          endpointId &&
            this.windows.get(this.activeWindowId ?? "")?.kind === "endpoint" &&
            endpointWindowId(endpointId) === this.activeWindowId,
        ),
      );
    });
  }

  private handlePointerDown(event: PointerEvent): void {
    if (!(event.target instanceof Element)) return;
    const article = event.target.closest<HTMLElement>("[data-window-id]");
    if (!article?.dataset.windowId) return;
    this.focusWindow(article.dataset.windowId);
    const handle = event.target.closest<HTMLElement>("[data-window-drag-handle]");
    if (
      !handle ||
      event.target.closest("button, input, textarea, select") ||
      this.compactLayout.matches
    ) {
      return;
    }
    const workspaceWindow = this.windows.get(article.dataset.windowId);
    if (!workspaceWindow) return;
    event.preventDefault();
    article.setPointerCapture(event.pointerId);
    this.drag = {
      pointerId: event.pointerId,
      windowId: workspaceWindow.id,
      startX: event.clientX,
      startY: event.clientY,
      originX: workspaceWindow.x,
      originY: workspaceWindow.y,
      element: article,
    };
    article.classList.add("dragging");
  }

  private handlePointerMove(event: PointerEvent): void {
    if (!this.drag || this.drag.pointerId !== event.pointerId) return;
    const workspaceWindow = this.windows.get(this.drag.windowId);
    if (!workspaceWindow) return;
    const next = this.clampPosition(
      this.drag.originX + event.clientX - this.drag.startX,
      this.drag.originY + event.clientY - this.drag.startY,
      workspaceWindow.width,
      workspaceWindow.height,
    );
    workspaceWindow.x = next.x;
    workspaceWindow.y = next.y;
    this.drag.element.style.setProperty("--window-x", `${next.x}px`);
    this.drag.element.style.setProperty("--window-y", `${next.y}px`);
  }

  private handlePointerUp(event: PointerEvent): void {
    if (!this.drag || this.drag.pointerId !== event.pointerId) return;
    this.drag.element.classList.remove("dragging");
    if (this.drag.element.hasPointerCapture(event.pointerId)) {
      this.drag.element.releasePointerCapture(event.pointerId);
    }
    this.drag = undefined;
    this.captureGeometry();
  }

  private captureGeometry(): void {
    if (this.compactLayout.matches) return;
    const rootBounds = this.root.getBoundingClientRect();
    this.root.querySelectorAll<HTMLElement>("[data-window-id]").forEach((element) => {
      const workspaceWindow = this.windows.get(element.dataset.windowId ?? "");
      if (!workspaceWindow || workspaceWindow.minimized) return;
      const bounds = element.getBoundingClientRect();
      if (bounds.width < 240 || bounds.height < 160) return;
      workspaceWindow.x = bounds.left - rootBounds.left;
      workspaceWindow.y = bounds.top - rootBounds.top;
      workspaceWindow.width = bounds.width;
      workspaceWindow.height = bounds.height;
    });
  }

  private endpointGeometry(_anchor?: WindowAnchor) {
    const rootWidth = this.root.clientWidth || globalThis.innerWidth;
    const rootHeight = this.root.clientHeight || globalThis.innerHeight;
    const width = Math.min(480, Math.max(340, rootWidth - 32));
    const height = Math.min(650, Math.max(420, rootHeight - 48));
    const slot = this.cascade++;
    const column = slot % 2;
    const row = Math.floor(slot / 2) % 5;
    const rightColumn = rootWidth - width - 18;
    const leftColumn = Math.max(18, rightColumn - width * 0.78);
    const next = this.clampPosition(
      column === 0 ? leftColumn : rightColumn,
      38 + row * 34,
      width,
      height,
    );
    this.zIndex += 1;
    return { ...next, width, height, zIndex: this.zIndex };
  }

  private backendGeometry() {
    const rootWidth = this.root.clientWidth || globalThis.innerWidth;
    const rootHeight = this.root.clientHeight || globalThis.innerHeight;
    const width = Math.min(560, Math.max(340, rootWidth - 32));
    const height = Math.min(700, Math.max(420, rootHeight - 48));
    const next = this.clampPosition(rootWidth - width - 38, 46, width, height);
    this.zIndex += 1;
    return { ...next, width, height, zIndex: this.zIndex };
  }

  private clampPosition(x: number, y: number, width: number, height: number) {
    const rootWidth = this.root.clientWidth || globalThis.innerWidth;
    const rootHeight = this.root.clientHeight || globalThis.innerHeight;
    return {
      x: Math.max(8, Math.min(x, Math.max(8, rootWidth - width - 8))),
      y: Math.max(8, Math.min(y, Math.max(8, rootHeight - height - 8))),
    };
  }
}

const endpointWindowId = (endpointId: string): string =>
  `endpoint:${endpointId}`;

const isWatchable = (method: string): boolean =>
  method === "GET" || method === "HEAD";

const parseJsonField = (value: string): JsonValue | undefined => {
  const trimmed = value.trim();
  if (!trimmed) return undefined;
  try {
    return parseJson(trimmed);
  } catch {
    return value;
  }
};

const parseJson = (value: string): JsonValue => {
  if (
    value.length > MAX_BACKEND_JSON_BYTES ||
    new TextEncoder().encode(value).byteLength > MAX_BACKEND_JSON_BYTES
  ) {
    throw new Error("JSON input is limited to 2 MB");
  }
  const parsed: unknown = JSON.parse(value);
  if (!isJsonValue(parsed, 0, { remaining: MAX_JSON_INPUT_NODES })) {
    throw new Error("Value must be finite, bounded JSON");
  }
  return parsed;
};

const isJsonValue = (
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
      value.every((item) => isJsonValue(item, depth + 1, budget))
    );
  }
  if (typeof value !== "object") return false;
  const entries = Object.entries(value);
  return (
    entries.length <= 300 &&
    entries.every(([, item]) => isJsonValue(item, depth + 1, budget))
  );
};

const inputError = (
  endpointId: string,
  message: string,
): ExecutionResult => ({
  endpointId,
  status: "422",
  body: { error: message },
  requestErrors: [message],
  responseErrors: [],
});

const errorMessage = (error: unknown, fallback: string): string =>
  error instanceof Error ? error.message : fallback;
