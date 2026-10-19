# Containers plugin, step 1: plan index

**Goal:** Ship step 1 of the Jensen Containers plugin: a files-only map, readiness checks with suggested edits, navigation between the map and the files, a status bar count, and the `topology` and `readiness` agent tools. It must need no new Jensen capability.

**Spec:** `docs/superpowers/specs/2026-10-19-containers-plugin-design.md`. Section 10 "Step 1" is the scope, and sections 3, 5, 6 and 7 hold the constraints.

**Branch:** Jensen provisions it. Approving part 1's plan cuts the task branch and its worktree, and parts 2 and 3 are the next plans of the same Jensen chain, on that branch. The chain lands as one pull request.

## Global Constraints

- **Repo**: `/home/fxxx/projects/jensen-containers`. Its first commit holds only `docs/superpowers/`. `main` serves an environment, so agents work only in the branch and worktree Jensen gives an approved plan.
- **Plugin manifest** (the `jensen` block in `package.json`):
  - `id: "dev.jensen.containers"`. This is a working id; the owner confirms it before the first store release.
  - display name `Containers`, `category: "devtools"`, `minAppVersion: "0.3.0"`.
  - `apiVersion` 1, written by the CLI.
- **SDK**: `@jensen-org/plugin-sdk` `^0.2.0`. The backend uses `jensen-plugin-backend` from `git = "https://github.com/jensen-org/plugin-sdk", tag = "v0.2.0"`, with Rust edition 2024, crate type `["cdylib", "rlib"]`, target `wasm32-unknown-unknown`, and release profile `opt-level = "s"`, `lto = true`, `strip = true`.
- **Toolchain**: the SDK template's. That means bun (`bun install`, `bun test`), TypeScript `^5.9`, Biome for lint, and esbuild through `jensen-plugin build`.
- **Permissions**: exactly `workspace: true`, `editor: "write"`, `graph: true`, `backend: true`, `fs: ["*.yaml", "*.yml"]`. No `network`, `settings`, `knowledge`, `git` or `theme`.
- **No new host imports**: the backend uses only `jensen-plugin-backend`'s `fs::read`, `fs::write` (never called) and `log`.
- **Hands off the project**: the plugin never writes a file into the project. The only project changes are edits the person accepts, applied through `editor.replaceRange` as one undoable change. Plugin state goes in `storage.load` and `storage.save`.
- **Discovery skips worktree folders**: any path under `.jensen/worktrees/` or `.claude/worktrees/`, plus `node_modules/`, `.git/` and `target/`.
- **Nothing is guessed**: a variable without an inline default (`${VAR}`, `$VAR`, `${VAR:?err}`) is reported as unresolved. Anything inferred carries `lead: true` and says so in its text.
- **Line and column numbers** in every interface are zero-based. Offsets are UTF-16 code units into the file's text, the way the SDK's decorations and `replaceRange` count them. The base of `editor.openFile`'s `line` is not documented: convert at its single call site, and pin the conversion with a test.
- **Readiness rule ids** (spec 6.4), exactly these five: `fixed_host_port`, `container_name`, `shared_fixed_name`, `same_project_name`, `unprovided_variable`.
  - `affects` is `["agents", "by_hand"]` for the first three and `["by_hand"]` for `same_project_name`.
  - `unprovided_variable` affects `["agents"]` and is always `lead: true`.
  - `same_project_name` and `unprovided_variable` carry no `edit`: editing the Compose file alone can't fix them, so their message explains what to do instead.
  - `same_project_name` is reported only when the project has at least one worktree folder under `.jensen/worktrees/` or `.claude/worktrees/`. A solo developer never sees it. If those folders can't be listed, it isn't reported.
- **Port fix text**: `"${<SERVICE>_PORT:-<today's host port>}:<container port>"`, with the service name upper-cased and `-` changed to `_`. A host IP prefix is kept: `"127.0.0.1:${DB_PORT:-5432}:5432"`. In long syntax, the edit replaces only the `published:` value, as `"${DB_PORT:-5432}"`.
- **Agent tools**: exactly two in step 1, `topology` and `readiness`. Each takes `{ checkout?: string, files?: string[] }` and returns `{ text: string }`, short plain text that ends with a suggested next step.
- **Cost**: no polling, and no network.
- **Verification**: `make check` exits 0 on every part. Jensen verifies each plan with it.

## Parts

| Part | Name | Deliverable | Tasks (rough) | Depends on |
|------|------|-------------|---------------|------------|
| 1 | Core and agent tools | The repo scaffold with `make check`. A Rust backend that loads Compose files with source positions, runs the five readiness rules and renders topology text. A minimal plugin that installs in Jensen and offers the `topology` and `readiness` agent tools. | 8 | none |
| 2 | Containers pane | A custom pane: the files-only map, a details panel for the selected service, the readiness list with "apply fix", and the code-map link. Shared discovery and loading used by every realm. A command to open it. | 6 | 1 |
| 3 | Editor and status bar | Readiness decorations in Compose files, a fix from the editor's right-click menu, "Show in map" from the editor and "Show in Containers" or "Check readiness" from the explorer. A status bar readiness count that opens the pane. The settings tab. | 6 | 1, 2 |

## Cross-Part Interfaces

### Part 1 produces

**Backend methods**, called with `app.backend.call(method, input)`, each answering JSON:
- `analyze(input: { files: string[]; checkout?: string }) -> AnalyzeResult`: for the UI, not an agent tool. `files` are project-relative Compose file paths. When `checkout` is given, each path is read as `<checkout>/<path>`.
- `topology(input: { checkout?: string; files?: string[] }) -> { text: string }`: an agent tool.
- `readiness(input: { checkout?: string; files?: string[] }) -> { text: string }`: an agent tool.

**Discovery inside the backend**: with no `files`, the backend reads, under the checkout root, the first of `compose.yaml`, `compose.yml`, `docker-compose.yaml` and `docker-compose.yml` that exists, follows `include:`, and reports in its `text` when none was found.

**TypeScript mirror of the result types**, in `src/backend.ts`. The Rust structs in `backend/src/model.rs` serialize to exactly these shapes, using camelCase serde.

```ts
export interface SourceRef { file: string; line: number; column: number }
export interface Span { from: number; to: number }
export interface PortBinding { hostIp?: string; host?: string; container: string; protocol?: string; fixed: boolean; source: SourceRef; span: Span }
export interface VolumeMount { kind: "named" | "bind" | "tmpfs"; source?: string; target: string; readOnly: boolean }
export interface ServiceNode { name: string; image?: string; build?: { context: string; dockerfile?: string }; ports: PortBinding[]; volumes: VolumeMount[]; networks: string[]; dependsOn: string[]; containerName?: string; profiles: string[]; source: SourceRef }
export interface ResourceNode { name: string; fixedName?: string; external: boolean; source: SourceRef }
export interface Edge { from: string; to: string; kind: "depends_on" | "network" | "volume" | "links" }
export interface ProjectModel { projectName: { value: string; origin: "name" | "folder" }; files: string[]; services: ServiceNode[]; volumes: ResourceNode[]; networks: ResourceNode[]; edges: Edge[] }
export type RuleId = "fixed_host_port" | "container_name" | "shared_fixed_name" | "same_project_name" | "unprovided_variable"
export interface TextEdit { file: string; span: Span; newText: string; title: string }
export interface Finding { rule: RuleId; service?: string; message: string; affects: Array<"agents" | "by_hand">; lead: boolean; source: SourceRef; span?: Span; edit?: TextEdit }
export interface Unresolved { variable: string; source: SourceRef }
export interface LoadError { file: string; message: string; source?: SourceRef }
export interface AnalyzeResult { model: ProjectModel; findings: Finding[]; unresolved: Unresolved[]; errors: LoadError[] }
export function analyze(app: App, input: { files: string[]; checkout?: string }): Promise<AnalyzeResult>
```

**Plugin entry**: `src/main.ts` exports `default class ContainersPlugin extends Plugin`. Each later part adds one module exporting `register<Thing>(plugin: ContainersPlugin): void`, and adds one call to it at the end of `onload()`. Part 2 adds `registerPane`; part 3 adds `registerEditor`, `registerStatus` and `registerSettings`. No part edits another part's module, except where an interface below says so.

### Part 2 produces

**Shared discovery and loading** (`src/project.ts`), usable from the headless copy and from the pane's own frame:
- `discoverComposeFiles(app: App, settings: Settings): Promise<string[]>`. Walks `app.files.list` from `"."`, skips the folders listed in Global Constraints, keeps YAML files whose top-level keys include `services`, adds `settings.extraComposePaths` that exist, and sorts the result.
  - If listing a folder is refused, it falls back to the four usual names at the root, plus the extra paths.
- `loadProject(app: App): Promise<AnalyzeResult>`. Reads the settings, runs discovery, then calls `analyze`.
- `export interface Settings { extraComposePaths: string[] }`, with default `{ extraComposePaths: [] }`, kept under the key `settings` of the plugin's data object (`loadData` / `saveData`).
- `readSettings(plugin: Plugin): Promise<Settings>` and `writeSettings(plugin: Plugin, settings: Settings): Promise<void>`.

**The pane:**
- Pane id `"containers"`, title `"Containers"`, icon `"container"`, singleton.
- `export interface ContainersPaneState { focus?: { file: string; service?: string }; view?: "map" | "readiness" }`.
- `openContainers(plugin: Plugin, state?: ContainersPaneState): Promise<void>` opens the pane, or focuses it and applies the state.
- Command id `"open-containers"`, named "Open Containers".

**Applying a fix**: `applyEdit(app: App, edit: TextEdit): Promise<boolean>`. It opens `edit.file` at the edit, checks that the buffer text at `edit.span` still matches what the finding saw, then applies one `replaceRange`. It returns false, and changes nothing, when the text no longer matches. Part 3 reuses it from the editor menu.

**Verify first in part 2**: check that `backend.call` and `fs.list` work from the custom pane's own frame, and not only from the headless copy. The SDK doesn't say. If they don't, stop and report it as an open question: the pane can't reach the headless copy, so the plan would change.

### Part 3 produces

No interface later parts rely on. It registers the editor decorations, the menus, the status bar item and the settings tab.
