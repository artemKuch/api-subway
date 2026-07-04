import type { ExecutionResult, VirtualRequest } from "../virtual-backend/types";

export type PayloadViewMode = "schema" | "json";

export interface WindowGeometry {
  x: number;
  y: number;
  width: number;
  height: number;
  zIndex: number;
}

interface WorkspaceWindowBase extends WindowGeometry {
  id: string;
  minimized: boolean;
}

export interface EndpointWorkspaceWindow extends WorkspaceWindowBase {
  kind: "endpoint";
  endpointId: string;
  request: VirtualRequest;
  bodyText: string;
  requestView: PayloadViewMode;
  responseView: PayloadViewMode;
  requestError?: string;
  response?: ExecutionResult;
  notice?: string;
}

export interface BackendWorkspaceWindow extends WorkspaceWindowBase {
  kind: "backend";
  jsonText: string;
  dirty: boolean;
  error?: string;
  notice?: string;
}

export type WorkspaceWindow =
  | EndpointWorkspaceWindow
  | BackendWorkspaceWindow;

export interface WindowAnchor {
  x: number;
  y: number;
}
