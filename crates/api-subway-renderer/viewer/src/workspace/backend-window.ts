import type { BackendWorkspaceWindow } from "./types";
import { escapeHtml, icon } from "./html";

export const renderBackendWindow = (
  window: BackendWorkspaceWindow,
  active: boolean,
): string =>
  `<article class="workspace-window backend-workspace-window ${active ? "active" : ""} ${window.minimized ? "minimized" : ""}" data-window-id="${escapeHtml(window.id)}" style="--window-x:${window.x}px;--window-y:${window.y}px;--window-width:${window.width}px;--window-height:${window.height}px;--window-z:${window.zIndex}">
    <header class="workspace-window-titlebar" data-window-drag-handle>
      <span class="window-drag-icon">${icon("drag")}</span>
      <span class="backend-title-icon">${icon("database")}</span>
      <strong>Virtual backend</strong>
      <span class="window-title-spacer"></span>
      <button type="button" data-window-action="minimize" aria-label="Minimize virtual backend">${icon("minimize")}</button>
      <button type="button" data-window-action="close" aria-label="Close virtual backend">${icon("close")}</button>
    </header>
    <div class="workspace-window-content">
      <div class="backend-editor-heading"><div><span>Editable JSON store</span><strong>Apply replaces the backend and resets endpoint windows</strong></div><span class="backend-dirty-state ${window.dirty ? "dirty" : ""}">${window.dirty ? "Unsaved changes" : "Synced"}</span></div>
      <textarea class="backend-json-editor" data-backend-json spellcheck="false" aria-label="Virtual backend JSON">${escapeHtml(window.jsonText)}</textarea>
      ${window.error ? `<div class="backend-editor-error" role="alert">${escapeHtml(window.error)}</div>` : ""}
      ${window.notice ? `<div class="window-live-notice"><span class="live-dot watching"></span>${escapeHtml(window.notice)}</div>` : ""}
      <div class="backend-window-actions">
        <button type="button" data-window-action="backend-apply" class="primary">Apply JSON</button>
        <button type="button" data-window-action="backend-reset">${icon("reset")}<span>Reset</span></button>
        <button type="button" data-window-action="backend-import">${icon("import")}<span>Import</span></button>
        <button type="button" data-window-action="backend-export">${icon("export")}<span>Export</span></button>
        <input type="file" data-backend-file accept="application/json,.json" class="sr-only">
      </div>
    </div>
  </article>`;
