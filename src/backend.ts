import type { App } from "@jensen-org/plugin-sdk";

// The shapes the Rust backend (backend/src/model.rs) answers with, as camelCase JSON.
//
// Conventions that hold for every interface below:
// - Lines and columns are zero-based.
// - Columns and offsets count UTF-16 code units, the way the editor and `replaceRange` count them.
//   Offsets count a `\r\n` line break as one position, as CodeMirror does.
// - Every path is relative to the checkout root (the folder `checkout` names, or the project root);
//   a caller that passed a checkout prefixes it to open a file.
// - An edge's `from` is always a service. Its `to` is a service for `depends_on` and `links`, a
//   network for `network` and a volume for `volume`.

export interface SourceRef {
  file: string;
  line: number;
  column: number;
}
export interface Span {
  from: number;
  to: number;
}
export interface PortBinding {
  hostIp?: string;
  host?: string;
  container: string;
  protocol?: string;
  fixed: boolean;
  source: SourceRef;
  span: Span;
}
export interface VolumeMount {
  kind: "named" | "bind" | "tmpfs";
  source?: string;
  target: string;
  readOnly: boolean;
}
export interface ServiceNode {
  name: string;
  image?: string;
  build?: { context: string; dockerfile?: string };
  ports: PortBinding[];
  volumes: VolumeMount[];
  networks: string[];
  dependsOn: string[];
  containerName?: string;
  profiles: string[];
  source: SourceRef;
}
export interface ResourceNode {
  name: string;
  fixedName?: string;
  external: boolean;
  source: SourceRef;
}
export interface Edge {
  from: string;
  to: string;
  kind: "depends_on" | "network" | "volume" | "links";
}
export interface ProjectModel {
  projectName: { value: string; origin: "name" | "folder" };
  files: string[];
  services: ServiceNode[];
  volumes: ResourceNode[];
  networks: ResourceNode[];
  edges: Edge[];
}
export type RuleId =
  | "fixed_host_port"
  | "container_name"
  | "shared_fixed_name"
  | "same_project_name"
  | "unprovided_variable";
export interface TextEdit {
  file: string;
  span: Span;
  newText: string;
  title: string;
}
export interface Finding {
  rule: RuleId;
  service?: string;
  message: string;
  affects: Array<"agents" | "by_hand">;
  lead: boolean;
  source: SourceRef;
  span?: Span;
  edit?: TextEdit;
}
export interface Unresolved {
  variable: string;
  source: SourceRef;
}
export interface LoadError {
  file: string;
  message: string;
  source?: SourceRef;
}
export interface AnalyzeResult {
  model: ProjectModel;
  findings: Finding[];
  unresolved: Unresolved[];
  errors: LoadError[];
}

/** Loads the Compose files of `input.files` (relative to the checkout root) and models them. */
export function analyze(
  app: App,
  input: { files: string[]; checkout?: string },
): Promise<AnalyzeResult> {
  return app.backend.call<AnalyzeResult>("analyze", input);
}
