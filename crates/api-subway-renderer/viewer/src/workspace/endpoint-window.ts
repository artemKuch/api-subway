import type {
  ApiSchema,
  Endpoint,
  ParameterContract,
  ParameterLocation,
} from "../types";
import type { VirtualOperation } from "../virtual-backend/types";
import type { EndpointWorkspaceWindow, PayloadViewMode } from "./types";
import { escapeHtml, icon, jsonFieldValue, methodClass } from "./html";
import {
  renderResponseJsonView,
  renderResponseSchemaView,
} from "./response-view";
import {
  renderRequestBodyFields,
  schemaTypeLabel,
} from "./schema-fields";

const locationLabels: Record<ParameterLocation, string> = {
  path: "Path",
  query: "Query",
  header: "Headers",
  cookie: "Cookies",
};

export const renderEndpointWindow = (
  window: EndpointWorkspaceWindow,
  endpoint: Endpoint,
  operation: VirtualOperation,
  schemas: Map<string, ApiSchema>,
  active: boolean,
): string => {
  const parameters = endpoint.contract?.request?.parameters ?? [];
  const bodyContract = endpoint.contract?.request?.bodies?.[0];
  const response = window.response;
  const errors = [
    ...(response?.requestErrors ?? []),
    ...(response?.responseErrors ?? []),
  ];
  const successful = response ? /^2\d\d$/.test(response.status) : false;

  return `<article class="workspace-window endpoint-workspace-window ${active ? "active" : ""} ${window.minimized ? "minimized" : ""}" data-window-id="${escapeHtml(window.id)}" data-endpoint-id="${escapeHtml(endpoint.id)}" data-method="${escapeHtml(endpoint.method)}" data-path="${escapeHtml(endpoint.path)}" style="--window-x:${window.x}px;--window-y:${window.y}px;--window-width:${window.width}px;--window-height:${window.height}px;--window-z:${window.zIndex}">
    <header class="workspace-window-titlebar" data-window-drag-handle>
      <span class="window-drag-icon">${icon("drag")}</span>
      <span class="endpoint-method ${methodClass(endpoint.method)}">${escapeHtml(endpoint.method)}</span>
      <strong>${escapeHtml(endpoint.path)}</strong>
      <span class="window-title-spacer"></span>
      <button type="button" data-window-action="minimize" aria-label="Minimize ${escapeHtml(endpoint.method)} ${escapeHtml(endpoint.path)}">${icon("minimize")}</button>
      <button type="button" data-window-action="close" aria-label="Close ${escapeHtml(endpoint.method)} ${escapeHtml(endpoint.path)}">${icon("close")}</button>
    </header>
    <div class="workspace-window-content">
      <section class="endpoint-request-section">
        <div class="request-title"><h3>Request</h3>${bodyContract ? renderPayloadViewSwitch("request", window.requestView) : ""}</div>
        ${renderParameters(window, parameters, schemas)}
        ${bodyContract ? (window.requestView === "schema" ? renderRequestBodyFields(window.request.body, bodyContract.schema_id, schemas, bodyContract.media_type) : renderBodyEditor(window, bodyContract.media_type)) : '<p class="window-empty-copy">No request body</p>'}
        ${window.requestError ? `<div class="window-validation-errors request-view-error" role="alert"><p>${escapeHtml(window.requestError)}</p></div>` : ""}
        <button type="button" class="run-request-button" data-window-action="run">${icon("play")}<span>Run</span></button>
      </section>
      <section class="endpoint-response-section">
        <div class="response-title"><h3>Response</h3>${response ? `<span class="response-status ${successful ? "success" : "error"}"><span></span>${escapeHtml(statusLabel(response.status))}</span>` : ""}${renderPayloadViewSwitch("response", window.responseView)}</div>
        ${window.responseView === "schema" ? renderResponseSchemaView(endpoint, operation.responseStatus, response, schemas) : renderResponseJsonView(response)}
        ${errors.length > 0 ? `<div class="window-validation-errors">${errors.map((error) => `<p>${escapeHtml(error)}</p>`).join("")}</div>` : response ? '<div class="window-validation-ok">✓ Request and response conform to the available contract</div>' : ""}
      </section>
      ${window.notice ? `<div class="window-live-notice"><span class="live-dot watching"></span>${escapeHtml(window.notice)}</div>` : ""}
    </div>
  </article>`;
};

const renderParameters = (
  window: EndpointWorkspaceWindow,
  parameters: ParameterContract[],
  schemas: Map<string, ApiSchema>,
): string => {
  if (!parameters?.length) return "";
  return (["path", "query", "header", "cookie"] as ParameterLocation[])
    .map((location) => {
      const fields = parameters.filter(
        (parameter) => parameter.location === location,
      );
      if (fields.length === 0) return "";
      return `<fieldset class="request-parameter-group"><legend>${locationLabels[location]}</legend>${fields
        .map(
          (parameter) =>
            `<label><span><strong>${escapeHtml(parameter.name)}${parameter.required ? " *" : ""}</strong><code>${escapeHtml(schemaTypeLabel(parameter.schema_id, schemas))}</code></span><input type="text" data-request-location="${location}" data-request-name="${escapeHtml(parameter.name)}" value="${escapeHtml(jsonFieldValue(window.request.parameters[location][parameter.name]))}" autocomplete="off" spellcheck="false"></label>`,
        )
        .join("")}</fieldset>`;
    })
    .join("");
};

const renderBodyEditor = (
  window: EndpointWorkspaceWindow,
  mediaType: string,
): string =>
  `<label class="request-body-editor request-json-view"><span class="request-json-heading"><strong>Body JSON</strong><code>${escapeHtml(mediaType)}</code></span><textarea data-request-body spellcheck="false" aria-label="Body JSON"${window.requestError ? ' aria-invalid="true"' : ""}>${escapeHtml(window.bodyText)}</textarea></label>`;

const renderPayloadViewSwitch = (
  scope: "request" | "response",
  view: PayloadViewMode,
): string => {
  const label = scope === "request" ? "Request body view" : "Response view";
  return `<div class="${scope}-view-switch payload-view-switch" role="group" aria-label="${label}"><button type="button" data-window-action="${scope}-schema" class="${view === "schema" ? "active" : ""}" aria-label="Show ${scope} as schema" aria-pressed="${view === "schema"}">${icon("schema")}<span>Schema</span></button><button type="button" data-window-action="${scope}-json" class="${view === "json" ? "active" : ""}" aria-label="Show ${scope} as JSON" aria-pressed="${view === "json"}">${icon("json")}<span>JSON</span></button></div>`;
};

const statusLabel = (status: string): string => {
  const labels: Record<string, string> = {
    "200": "200 OK",
    "201": "201 Created",
    "202": "202 Accepted",
    "204": "204 No Content",
    "400": "400 Bad Request",
    "404": "404 Not Found",
    "422": "422 Unprocessable Entity",
    "500": "500 Internal Error",
  };
  return labels[status] ?? status;
};
