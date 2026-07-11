import type { ApiMap, DependencyKind } from "./types";
import { VirtualBackendEngine } from "./virtual-backend/engine";
import { planVirtualBackend } from "./virtual-backend/planner";
import { VirtualBackendStore } from "./virtual-backend/store";
import { WorkspaceWindowManager } from "./workspace/window-manager";

type ViewBox = [number, number, number, number];

const requireElement = <T extends Element>(selector: string): T => {
  const element = document.querySelector<T>(selector);
  if (!element) throw new Error(`api-subway viewer is missing ${selector}`);
  return element;
};

const map = JSON.parse(
  requireElement<HTMLScriptElement>("#api-map-data").textContent ?? "{}",
) as ApiMap;
const svg = requireElement<SVGSVGElement>(".api-map");
const viewport = requireElement<HTMLElement>("#map-viewport");
const workspaceRoot = requireElement<HTMLElement>("#workspace-layer");
const search = requireElement<HTMLInputElement>("#search");
const methodFilter = requireElement<HTMLSelectElement>("#method-filter");
const resultCount = requireElement<HTMLElement>("#result-count");
const openBackendButton =
  requireElement<HTMLButtonElement>("#open-backend");
const resetBackendButton =
  requireElement<HTMLButtonElement>("#reset-backend");
const compactLayout = globalThis.matchMedia("(max-width: 900px)");

const parsedViewBox = (svg.getAttribute("viewBox") ?? "0 0 1200 800")
  .split(/\s+/)
  .map(Number);
const originalViewBox: ViewBox = [
  parsedViewBox[0] ?? 0,
  parsedViewBox[1] ?? 0,
  parsedViewBox[2] ?? 1200,
  parsedViewBox[3] ?? 800,
];
let viewBox: ViewBox = [...originalViewBox];
let activeKind: DependencyKind | "all" = "all";
let mapDrag:
  | {
      pointerId: number;
      startX: number;
      startY: number;
      viewX: number;
      viewY: number;
      moved: boolean;
    }
  | undefined;

const dependencyById = new Map(
  map.dependencies.map((dependency) => [dependency.id, dependency]),
);

const alignMapForViewport = (): void => {
  if (compactLayout.matches) {
    const scale = 0.82;
    const width = originalViewBox[2] * scale;
    const height = originalViewBox[3] * scale;
    viewBox = [
      originalViewBox[0] + (originalViewBox[2] - width) / 2,
      originalViewBox[1],
      width,
      height,
    ];
  } else {
    viewBox = [...originalViewBox];
  }
  svg.setAttribute("viewBox", viewBox.join(" "));
  svg.setAttribute(
    "preserveAspectRatio",
    compactLayout.matches ? "xMidYMin meet" : "xMidYMid meet",
  );
};

alignMapForViewport();
compactLayout.addEventListener("change", alignMapForViewport);

for (const method of [
  ...new Set(map.endpoints.map((endpoint) => endpoint.method)),
]) {
  const option = document.createElement("option");
  option.value = method;
  option.textContent = method;
  methodFilter.append(option);
}

const initialBackend = planVirtualBackend(map);
const store = new VirtualBackendStore(initialBackend);
const engine = new VirtualBackendEngine(map, store);
const workspace = new WorkspaceWindowManager(
  workspaceRoot,
  map,
  store,
  engine,
);

const applyFilters = (): void => {
  const query = search.value.trim().toLocaleLowerCase();
  const method = methodFilter.value;
  let visible = 0;
  svg.querySelectorAll<SVGGElement>(".station").forEach((station) => {
    const matchesText =
      !query ||
      `${station.dataset.method ?? ""} ${station.dataset.path ?? ""}`
        .toLocaleLowerCase()
        .includes(query);
    const matchesMethod = method === "all" || station.dataset.method === method;
    const dependencies = (station.dataset.dependencies ?? "").split(",");
    const matchesKind =
      activeKind === "all" ||
      dependencies.some(
        (dependencyId) => dependencyById.get(dependencyId)?.kind === activeKind,
      );
    const matches = matchesText && matchesMethod && matchesKind;
    station.classList.toggle("is-muted", !matches);
    if (matches) visible += 1;
  });
  svg
    .querySelectorAll<SVGElement>(
      ".dependency-rail,.dependency-line,.legend-item",
    )
    .forEach((line) => {
      line.classList.toggle(
        "is-muted",
        activeKind !== "all" && line.dataset.kind !== activeKind,
      );
    });
  resultCount.textContent = `${visible} of ${map.endpoints.length} stations`;
};

const setViewBox = (): void => svg.setAttribute("viewBox", viewBox.join(" "));

const zoomAt = (factor: number, clientX: number, clientY: number): void => {
  const bounds = svg.getBoundingClientRect();
  if (bounds.width === 0 || bounds.height === 0) return;
  const pointX =
    viewBox[0] + ((clientX - bounds.left) / bounds.width) * viewBox[2];
  const pointY =
    viewBox[1] + ((clientY - bounds.top) / bounds.height) * viewBox[3];
  const nextWidth = Math.min(
    originalViewBox[2] * 2,
    Math.max(originalViewBox[2] * 0.22, viewBox[2] * factor),
  );
  const nextHeight = nextWidth * (viewBox[3] / viewBox[2]);
  const ratioX = (pointX - viewBox[0]) / viewBox[2];
  const ratioY = (pointY - viewBox[1]) / viewBox[3];
  viewBox = [
    pointX - ratioX * nextWidth,
    pointY - ratioY * nextHeight,
    nextWidth,
    nextHeight,
  ];
  setViewBox();
};

const zoomCenter = (factor: number): void => {
  const bounds = svg.getBoundingClientRect();
  zoomAt(factor, bounds.left + bounds.width / 2, bounds.top + bounds.height / 2);
};

const openStation = (
  station: SVGGElement,
  clientX?: number,
  clientY?: number,
): void => {
  const endpointId = station.dataset.endpointId;
  if (!endpointId) return;
  const rootBounds = workspaceRoot.getBoundingClientRect();
  const stationBounds = station.getBoundingClientRect();
  workspace.openEndpoint(endpointId, {
    x: (clientX ?? stationBounds.right) - rootBounds.left,
    y:
      (clientY ?? stationBounds.top + stationBounds.height / 2) -
      rootBounds.top,
  });
};

search.addEventListener("input", applyFilters);
methodFilter.addEventListener("change", applyFilters);

requireElement("#kind-filters").addEventListener("click", (event) => {
  if (!(event.target instanceof HTMLButtonElement)) return;
  const kind = event.target.dataset.kind as DependencyKind | "all" | undefined;
  if (!kind) return;
  activeKind = kind;
  document
    .querySelectorAll("#kind-filters button")
    .forEach((button) => button.classList.toggle("active", button === event.target));
  applyFilters();
});

requireElement("#theme-toggle").addEventListener("click", () => {
  const root = document.documentElement;
  root.dataset.theme = root.dataset.theme === "paper" ? "midnight" : "paper";
});

requireElement("#zoom-in").addEventListener("click", () => zoomCenter(0.82));
requireElement("#zoom-out").addEventListener("click", () => zoomCenter(1.22));
requireElement("#fit-map").addEventListener("click", () => {
  alignMapForViewport();
});

openBackendButton.addEventListener("click", () => workspace.openBackend());
resetBackendButton.addEventListener("click", () => workspace.resetBackend());

svg.addEventListener("click", (event) => {
  if (mapDrag?.moved || !(event.target instanceof Element)) return;
  const station = event.target.closest<SVGGElement>(".station");
  if (station) openStation(station, event.clientX, event.clientY);
});

svg.addEventListener("keydown", (event) => {
  if (!(event.target instanceof Element)) return;
  const station = event.target.closest<SVGGElement>(".station");
  if (!station || (event.key !== "Enter" && event.key !== " ")) return;
  event.preventDefault();
  openStation(station);
});

viewport.addEventListener("wheel", (event) => {
  event.preventDefault();
  zoomAt(event.deltaY > 0 ? 1.1 : 0.9, event.clientX, event.clientY);
}, { passive: false });

viewport.addEventListener("pointerdown", (event) => {
  if (
    event.button !== 0 ||
    !(event.target instanceof Element) ||
    event.target.closest(".station")
  ) {
    return;
  }
  viewport.setPointerCapture(event.pointerId);
  mapDrag = {
    pointerId: event.pointerId,
    startX: event.clientX,
    startY: event.clientY,
    viewX: viewBox[0],
    viewY: viewBox[1],
    moved: false,
  };
  viewport.classList.add("dragging");
});

viewport.addEventListener("pointermove", (event) => {
  if (!mapDrag || mapDrag.pointerId !== event.pointerId) return;
  const bounds = svg.getBoundingClientRect();
  const deltaX = event.clientX - mapDrag.startX;
  const deltaY = event.clientY - mapDrag.startY;
  if (Math.abs(deltaX) + Math.abs(deltaY) > 3) mapDrag.moved = true;
  viewBox[0] = mapDrag.viewX - (deltaX / bounds.width) * viewBox[2];
  viewBox[1] = mapDrag.viewY - (deltaY / bounds.height) * viewBox[3];
  setViewBox();
});

const endMapDrag = (event: PointerEvent): void => {
  if (!mapDrag || mapDrag.pointerId !== event.pointerId) return;
  if (viewport.hasPointerCapture(event.pointerId)) {
    viewport.releasePointerCapture(event.pointerId);
  }
  viewport.classList.remove("dragging");
  const moved = mapDrag.moved;
  mapDrag = moved
    ? {
        pointerId: -1,
        startX: 0,
        startY: 0,
        viewX: 0,
        viewY: 0,
        moved: true,
      }
    : undefined;
  if (moved) setTimeout(() => (mapDrag = undefined), 0);
};

viewport.addEventListener("pointerup", endMapDrag);
viewport.addEventListener("pointercancel", endMapDrag);

document.addEventListener("keydown", (event) => {
  const activeElement = document.activeElement;
  const typing =
    activeElement instanceof HTMLInputElement ||
    activeElement instanceof HTMLTextAreaElement ||
    activeElement instanceof HTMLSelectElement;
  if (event.key === "/" && !typing) {
    event.preventDefault();
    search.focus();
  }
});

applyFilters();
