import type { JsonValue } from "../schema-simulator";

export const escapeHtml = (value: string): string =>
  value.replace(
    /[&<>"']/g,
    (character) =>
      ({
        "&": "&amp;",
        "<": "&lt;",
        ">": "&gt;",
        '"': "&quot;",
        "'": "&#39;",
      })[character] ?? character,
  );

export const methodClass = (method: string): string => {
  const token = method.toLocaleLowerCase().replace(/[^a-z0-9_-]/g, "-");
  return `method-${token || "unknown"}`;
};

export const prettyJson = (value: JsonValue | object): string =>
  JSON.stringify(value, null, 2) ?? "null";

export const icon = (
  name:
    | "close"
    | "minimize"
    | "play"
    | "database"
    | "reset"
    | "import"
    | "export"
    | "drag"
    | "schema"
    | "json",
): string => {
  const paths: Record<typeof name, string> = {
    close: '<path d="M5 5l14 14M19 5L5 19"/>',
    minimize: '<path d="M5 12h14"/>',
    play: '<path d="m8 5 11 7-11 7Z"/>',
    database:
      '<ellipse cx="12" cy="5" rx="7" ry="3"/><path d="M5 5v6c0 1.7 3.1 3 7 3s7-1.3 7-3V5M5 11v6c0 1.7 3.1 3 7 3s7-1.3 7-3v-6"/>',
    reset: '<path d="M4 7v5h5M5.6 16a8 8 0 1 0 .4-9l-2 2"/>',
    import: '<path d="M12 3v12m0 0 4-4m-4 4-4-4M5 18v3h14v-3"/>',
    export: '<path d="M12 21V9m0 0 4 4m-4-4-4 4M5 6V3h14v3"/>',
    drag: '<circle cx="8" cy="6" r="1"/><circle cx="16" cy="6" r="1"/><circle cx="8" cy="12" r="1"/><circle cx="16" cy="12" r="1"/><circle cx="8" cy="18" r="1"/><circle cx="16" cy="18" r="1"/>',
    schema:
      '<rect x="4" y="4" width="6" height="5" rx="1"/><rect x="14" y="15" width="6" height="5" rx="1"/><path d="M10 6.5h4a2 2 0 0 1 2 2V15M7 9v3a3 3 0 0 0 3 3h4"/>',
    json: '<path d="M8 3H6a2 2 0 0 0-2 2v4a2 2 0 0 1-2 2 2 2 0 0 1 2 2v6a2 2 0 0 0 2 2h2M16 3h2a2 2 0 0 1 2 2v4a2 2 0 0 0 2 2 2 2 0 0 0-2 2v6a2 2 0 0 1-2 2h-2"/>',
  };
  return `<svg class="ui-icon" viewBox="0 0 24 24" aria-hidden="true">${paths[name]}</svg>`;
};

export const jsonFieldValue = (value: JsonValue | undefined): string => {
  if (typeof value === "string") return value;
  return value === undefined ? "" : prettyJson(value);
};
