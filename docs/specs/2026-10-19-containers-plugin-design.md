# Jensen Containers plugin: design

Date: 2026-10-19. Status: approved by the owner on 2026-10-19.

Labels used below:
- **Decided** means the owner chose it during the brainstorm.
- **My call** means the designer chose it and the owner has not explicitly confirmed it.
- Everything else is fact gathered from the Jensen SDK, the Jensen docs, or the horus repo.

## 1. Intent

Jensen runs several AI agents on one project at the same time, each in its own git worktree. Once a project runs containers and dev servers, that gets hard to follow: every agent can have its own copy of the stack, copies fight over ports, things crash quietly, and containers and volumes stay behind after their worktree is gone.

The plugin gives the person following this work a clear picture of the project's running stack. That means what the project's files declare, even when nothing runs, and what is actually running for the project across every copy. It raises an alert when something needs the person and offers a safe cleanup. It also gives agents a few short, targeted tools to understand what's wrong with their copy.

Constraints from the owner:
- It is published to the Jensen plugin store and must suit any Jensen user's project. It must not be tied to the owner's setup.
- horus (`../horus`) is the first real project it will be tested on, and nothing may depend on it.
- The owner hasn't hit this pain yet; they expect it once horus runs containers under several agents. Early validation on a real project matters for that reason.
- During development, features get validated, invalidated or added. Section 11 maps the owner's original 35-feature list onto this design.

## 2. Who it serves

- **The person following multi-agent work**: wants to know at a glance what runs where, and whether anything needs them.
- **A solo developer with no agents**: gets the map, the notes in Compose files and the status bar, with nothing about sessions. The plugin must be useful to them on its own.
- **Agents in Jensen sessions**: need short answers about their own copy so they don't burn context on raw output.
- **The Jensen IDE developer**: receives the capability requests in section 9. The owner works closely with them and can ask for SDK additions, so a missing capability is a request to write up, not a hard limit.

## 3. Platform facts that shape the design

### The plugin SDK (protocol v1)

- **Panes** are either declarative, a node tree Jensen draws, or custom, the plugin's own DOM in a sandboxed frame with Jensen's theme tokens as CSS variables. There is no graph node, so the map needs a custom pane. The same `main.js` runs headless for logic and again inside each custom pane's frame, with registrations skipped in the pane copy. Nothing in the SDK lets two panes talk to each other directly.
- **The editor**: decorations of kind `mark` (range, tone, tooltip), `line`, `gutter` (text, tone, tooltip) and `hint` (inline text); `openFile(path, line, column)`; and `replaceRange`, which edits the *active* buffer as one undoable change labelled with the plugin id (needs `editor: "write"`).
- **Other surfaces**: context menus on the explorer, editor, panes and tabs; a status bar item (text, icon, tooltip, tone, click); commands with hotkeys; a settings tab; confirm, choose, prompt and suggest dialogs.
- **Toasts** (`ui.notice`) take only a message, a severity and a timeout, with no buttons.
- **Fenced-block renderers** for markdown notes return HTML, which Jensen sanitizes.
- **Files**: `fs.*` (read, write, list, stat, watch) works only inside the granted scopes: a folder, a file type such as `*.yaml`, or everything (`.`). Scopes are always read *and* write, and a path that escapes through a symlink is refused. `fs.watch` delivers change events.
- **Code map, knowledge and git**: `graph.*` (explainService, dependencies, impact, query), `knowledge.*` (search, ingest) and `git.*` (history, semanticDiff), each behind its own permission.
- **Network**: `net.fetch` reaches only https hosts listed in the manifest.
- **Storage**: the plugin's own `storage.load` and `storage.save`.
- **What's missing**: no API for sessions, plans, worktrees, Run or containers; no process or socket access; no event when a worktree or session goes away.

### The WebAssembly backend

- Its only host imports are `fs_read`, `fs_write`, `fs_delete` and `log`. It **cannot list a directory**, and has no network, clock or process access.
- Each call has time, memory and I/O limits; their values are undocumented.
- **Agent tools** are backend methods declared in the manifest, at most 32. Agents reach them through Jensen's own MCP server as `<plugin_id>_<tool>`, via `list_plugin_tools` and `call_plugin_tool`, even when Jensen's window is closed. That server is already registered for every assistant, so plugin tools need no per-agent setup.
- A release ships only `main.js`, a README and one `.wasm` file. A plugin cannot ship or start a native binary.

### Jensen's Run (built in)

- **Detection**: it finds services from package scripts, Makefiles, Procfiles, Compose files and language runners. Users correct detection in `servers.yaml`.
- **The agents' `run` tool** has the actions list, start, status, logs and stop, plus `since` for incremental answers. Logs default to the last 80 lines. Each run belongs to a session: it is stopped when the session closes and swept if the session dies. A started service gets **its port shifted into the session's own band**. Agents are refused when they start servers, watchers or containers from the shell.
- **For a Compose service**, the IDE developer confirmed that Run's sandbox runs `nerdctl compose -f <file> -p jensen-<session> up -d --build <service>`. On Linux that happens directly on the host; on macOS, through a VM. What follows from this:
  - **Separate per session**: each session has its own Compose project, `jensen-<session>`. That separates its containers, its default network and its project-prefixed volumes, and the project label names the session.
  - **Still shared between sessions**: anything with a fixed name. That means `container_name`, and volumes or networks with a fixed `name:` or marked `external`. A fixed top-level `name:` is overridden by `-p`.
  - **No `--env-file`**: variables that a project passes through its own scripts (horus's `make up` uses `--env-file infra/versions.env`) are not set, unless a `.env` sits next to the Compose file.
  - **Ports**: the command shifts no ports. A fixed host port such as `5432:5432` is bound on the host by every session, and on Linux it also competes with the person's own Docker. This is inferred from the command as relayed, and confirming it is part of R5.
  - **Images**: `--build` builds images per session, which outlive the session unless something removes them.
- **The app's Run drawer** has run, stop and restart per service, live logs, and a "Started outside Jensen" list for services that answer on a port without having been started by Jensen.
- **Landing**: Jensen refuses to land a branch while a server still runs from its worktree.
- **The sandbox** (`jensen sandbox setup`): agents' servers for a project with a Dockerfile or Compose file run inside containerd, never on the host. macOS needs a small Linux VM. The sandbox is not installed on the owner's machine.

### Worktrees, sessions and plans

- **Worktrees** live at `.jensen/worktrees/<name>` by default, or `.claude/worktrees/<name>`. Jensen creates them when a plan is approved and removes them after landing or when a session retires.
- **From the terminal**, `jensen worktree list <path>` prints JSON with `path`, `branch`, `head`, `is_primary`, `locked`, `merged` and `idle_days`. A plugin can't run it.
- **Plans**: files at `.jensen/plans/<id>.md` record `session_id` and the branch in their frontmatter.
- **The app** shows each session with a status: Needs input, Blocked, Working, Idle or Finished.

### Other facts

- **Code map**: Jensen's map covers files, imports, and the services and layers an assistant proposed and the user confirmed. It does not cover running infrastructure.
- **Labelling inferences**: Jensen labels anything it infers rather than reads as a lead ("Honest by design"). The plugin follows the same rule.
- **MCP servers**: Jensen does not manage other MCP servers for agents; each agent CLI has its own config.
- **`collapse_repeats` in jensen-harness**: it only merges identical consecutive lines, and its crate is unlicensed and unpublished, so it is not reused. Its sibling idea, compact output plus a handle to fetch the raw text, is worth copying.
- **Compose project names and labels**: the default project name is the folder of the first Compose file. Compose labels containers with the project, the service, `project.working_dir`, `project.config_files` and `config-hash`. A named volume is prefixed with the project name unless it sets `name:` or `external: true`. `container_name` stops a second copy from starting.

## 4. Decisions

1. **Build on Run** (decided). Run stays in charge of agents' stacks: starting, stopping, isolation, port bands, and cleanup when a session ends. The plugin never starts or stops anything behind Run's back.
2. **The plugin is the picture, the cleanup and the diagnosis** (decided).
3. **No companion native helper** (decided). A helper with its own Docker access would be a second path around Jensen's guard, and would need setting up in every agent CLI. Instead Jensen is asked to let plugins read container and Run state (section 9).
4. **Isolation by default** (decided). Each agent gets its own copy, some services can be shared, and this is configurable per project and per task. It is a feature request for Run, not plugin code (R5).
5. **Suggest edits, never make them silently** (decided). When a project's files stop copies from running side by side, the plugin explains why and proposes a one-time edit. Nothing changes until the person accepts.
6. **The layout** (decided) is one pane: a rail of copies, the map, and a details panel. Section 6.1 describes it.
7. **Alerts** (decided) interrupt only when something needs the person. Section 6.3 gives the rule.
8. **Five agent tools** (decided): diagnose, wait, logs, topology and readiness. Section 6.6.
9. **Services that aren't containers** (decided), such as dev servers started from a Makefile, appear on the map too, taken from Run's detection.
10. **Ship in two steps** (decided). Section 10.
11. **Not invasive, and fits the workflow** (decided). Section 5 turns this into rules.
12. **Cleanup has three owners** (decided). An agent removes its own session's stack when it's done. Jensen removes a session's stack automatically when it removes that session's worktree. The person handles everything else. Section 6.5.
13. **Technology** (my call): TypeScript on `@jensen-org/plugin-sdk` for the plugin, plus a Rust crate on `jensen-plugin-backend`, built for `wasm32-unknown-unknown`, for parsing, readiness rules, log compaction and diagnosis. The panes call it through `backend.call` and the agent tools are its methods, so both run the same code. The crate also builds natively, so its logic is tested with `cargo test`.

## 5. Goals, non-goals, anti-goals

**Goals**
- G1. At a glance, know what's running for this project in every copy, and whether anything needs you.
- G2. Understand the project's infrastructure from its files alone, with nothing running.
- G3. Spot and fix what stops copies from running side by side.
- G4. Clean up what removed worktrees and sessions left behind, safely.
- G5. Let an agent find out what's wrong with its copy in one or two calls, with small answers.

**Non-goals**
- Managing the lifecycle of agents' stacks, which is Run's job.
- Being a general container manager like Docker Desktop, Portainer or Lens. Anything the plugin can't tie to the project is out of scope.
- Production or remote infrastructure.
- In the first version: Kubernetes, Helm, kustomize, Nomad, Quadlet, Tilt, Skaffold, docker-bake and devcontainers. They come later, and only if users ask (my call).
- A terminal or exec into containers.
- Comparing two copies side by side. The owner expects that to be rare.

**Anti-goals.** The owner would uninstall it if it became too invasive or didn't fit their workflow. The rules that follow from that are my call:
- **Quiet by default.** Toasts only on the rule in 6.3, and each kind of alert can be switched off in the plugin's settings.
- **Hands off the project.** It never edits a file without a yes, and writes nothing into the repository: no config file, no `.gitignore` entry, no index file. Its own state lives in plugin storage.
- **Narrow permissions.** It asks for the least the SDK allows, and read-only access is requested (R7).
- **Light on the machine.** It follows Jensen's events rather than polling, and does no background work beyond what the status bar needs.
- **Fits any layout.** It finds Compose files wherever they live (horus keeps its file in `infra/local`), handles several files and profiles, and lets the person point it at files it missed. That setting lives in plugin storage, not the repo. Env files (`.env`, `env_file:`, `--env-file`) are read only once the plugin can get read-only access to them (R7). Until then, a variable without an inline default is shown as unresolved, never guessed (my call).
- **Works without agents.** A solo developer sees no session vocabulary.
- **Doesn't change how anyone runs things.** People keep starting their own stack their way (`make up`, `docker compose`, a script). The plugin watches. Run's rules apply to agents only.

## 6. The experience

### 6.1 The Containers pane

One pane, registered as a single instance, made of three parts.

**The rail of copies**, on the left. A copy is one instance of the project's stack:
- **You**: anything started from the primary checkout, whether by hand, from Run's drawer, or listed by Run as "Started outside Jensen".
- **One row per agent session**, showing its number, branch, plan title and Jensen status (Working, Idle, Finished).
- **Shared**: services that several copies use. This row appears only once Run supports sharing (R5).
- **Leftovers**: section 6.5.

Each row carries a one-line summary ("api crashed", "5 of 5 up") and a row of status dots, one per service. Rows with problems move to the top with a colored edge. A problem the person hasn't looked at keeps a dot until they open that copy or its details. The seen state lives in plugin storage.

**The map**, in the middle:
- **What's drawn**: the services, both containers and the processes Run detects, plus volumes and published ports, as the files declare them.
- **Where links come from**: for containers, Compose `depends_on`, networks and links. Links to and from non-container services can only be inferred, so they are drawn differently and labelled as inferred. Examples are a running container's environment (from R2), or connection strings in files the plugin may read.
- **Which files**: the files-only map is built from the primary checkout's files. File discovery skips worktree folders (`.jensen/worktrees/`, `.claude/worktrees/`) so their copies of the Compose file don't appear twice. Once a copy is selected, the map may be drawn from that copy's own files where the plugin can read them, so an agent's Compose changes show on its copy (my call).
- **With a copy selected**, each service shows that copy's state: running; starting or unhealthy; crashed; stopped; declared but not running (dashed); running but not in the files (marked separately); shared.
- **With nothing selected**, the map shows the files only. That is the generic view, and the whole view for a solo developer.
- **A failing service gets a visible ring**, so it stands out even when the pane is small.

**The details panel**, on the right, for the selected service in the selected copy:
- **What it shows**: state, restarts, ports (including the shifted host port), and the most frequent error with its count and one example, stack traces folded.
- **Links**:
  - the logs;
  - the file and line that define the service (`editor.openFile`);
  - the code service its Dockerfile builds, from Jensen's code map ("built from `api/`, the api service in your architecture");
  - **Open session** (R6);
  - **Ask the agent**, which hands the error to that session (R6).
- **Actions**:
  - For copies Run manages, start, stop and restart call Run (R4).
  - For containers started outside Jensen, stop and restart go through Jensen after a confirmation (R4). The plugin never starts them, because only the person's own tooling knows how (my call).
  - There is no exec.

The owner chose this layout over two alternatives: one lane per copy (too tall, needs scrolling) and a grid of copies against services under the map. The mockups for all three are in section 12.

### 6.2 Around Jensen

- **Status bar**: always shows the worst state across copies ("1 crashed · 1 unhealthy"), in red or amber. Clicking it opens the pane on that problem. In the first version, before live state exists, it shows the readiness count instead.
- **Compose files in the editor**: a gutter badge and a short hint on each service line ("crashed in #15"). Readiness problems get a `mark` with a tooltip explaining them. The fix sits in the editor's right-click menu, and applying it opens the file at the line and makes one undoable `replaceRange` edit. A fix button on the line itself would need decorations with actions (R6).
- **Explorer menu** on Compose files: "Show in Containers" and "Check readiness". Up and Down are left to Run.
- **Editor menu**: "Show in map".

### 6.3 When alerts fire

The rule (decided):
- **Interrupt with a toast** when something needs the person:
  - their own copy breaks;
  - a shared service breaks;
  - an agent's copy is still broken after that agent has gone Idle or Finished;
  - leftovers pile up. This one fires at most once a day (my call).
- **Stay quiet otherwise.** An agent's copy that breaks while the agent is Working shows only in the rail, the map and the status bar. The agent sees it too, and is probably already fixing it.

"Breaks" means one of: exited with a non-zero code, a restart loop, a failing health check, or killed for running out of memory. The thresholds, such as how long a check must fail, are the builder's call.

A toast names whose copy it is and what its agent is doing ("api crashed in #15 · fix/15-login-timeout. Its agent is idle."). Toasts can't carry an Open button today (R6), so the status bar is the way in. Each kind of alert can be switched off in the settings tab.

### 6.4 Readiness

Readiness lists what stops a copy of the stack from running, or from running beside another copy. Each rule says which copies it affects. "Agents" means copies Run starts with `-p jensen-<session>`; "by hand" means copies the person starts from several checkouts.

- **Fixed host port** (`"5432:5432"`): affects agents and copies started by hand. On Linux it also collides with the person's own stack, because the sandbox binds the same host ports as Docker.
- **`container_name`**: affects agents and copies started by hand.
- **Volumes or networks with a fixed `name:`, or marked `external`**: every copy shares the same data or network. Affects agents and copies started by hand.
- **A fixed top-level `name:`, or the same folder-derived project name in every checkout** (horus's `local`): affects copies started by hand only, since Run overrides it with `-p`.
- **Variables with no default that nothing provides when Run starts the stack**: Run passes no `--env-file`. These variables come out empty, so the stack may not build or start under Run at all. With YAML-only access the plugin can't see whether a `.env` sits next to the file, so this finding is always labelled a lead.

The builder may add further rules, but each must be read from the files, or labelled as a lead when it's a guess.

For each problem, the plugin explains it in plain words and proposes the exact edit, for example `"${DB_PORT:-5432}:5432"`. It applies the edit only after the person accepts, as described in 6.2.

Agents get the same findings from the `readiness` tool, so an agent can make the change as part of its task, under the usual plan approval.

Readiness and Run fit together. As relayed, Run shifts no Compose ports. Making a port a variable (`"${DB_PORT:-5432}:5432"`) is what lets each copy get its own port, but only once Run sets that variable per session (R5). The variable names in the suggested edits must match the convention Run adopts. Until it exists, suggestions use a `<SERVICE>_PORT` default that keeps today's port (my call).

Each finding says plainly what the edit does today. It lets a second copy started by hand use another port when the person sets the variable, and it does the same for agents once R5 lands. The tooltip in the `attention.html` mockup ("Run can't give each copy its own port here") is superseded by this section.

### 6.5 Cleanup

Cleanup has three owners (decided). Most leftovers come from what Run creates per session under `jensen-<session>`: containers, a network, volumes, and the images `--build` makes. So most cleanup happens at the source, and the person only handles what's left.

1. **The agent cleans up its own copy.** When it's done, or to free space mid-task, an agent can remove everything its own session created: containers, network, volumes and images under `jensen-<session>`.
   - It can never reach the primary checkout's data or another session's copy, because the scope is the caller's session.
   - The preferred home is Run's `run` tool, as a remove action, since Run created the stack (R5).
   - The fallback is a sixth plugin tool, `cleanup`, limited to the caller's session, through R3 (caller identity) and R4 (my call).
   - The agent instructions extend Run's existing "stop what you started" to "remove what you built when you're done".
2. **Jensen removes a session's stack together with its worktree.** When Jensen removes a session's worktree, after landing or when the session retires, it also removes that session's Compose project, volumes and images included (R5).
   - The stack lives exactly as long as the worktree (my call, mirroring Jensen's own rule). A session that retires with unpushed or uncommitted work keeps its worktree, so its stack stays too.
3. **The person handles the rest, in the pane**: anything tied to the project whose session no longer exists (for example, a session that died before cleaning up), and anything started by hand.
   - The plugin always shows the list with sizes first, and the person ticks what goes.
   - It never touches a running copy, the primary checkout's data, or anything it can't tie to the project.
   - Removal goes through Jensen (R4), and covers both Docker and Jensen's sandbox.

The "leftovers pile up" toast (6.3) only concerns case 3.

### 6.6 Agent tools

Principles:
- Answers are short text, not raw JSON.
- Repeated lines are counted, errors are grouped into templates with one example each, stack traces are folded, and long results are paged with a cursor.
- Anything cut short comes with a handle to fetch the raw text.
- Every answer ends with a suggested next step.
- Inferences are labelled as leads.

By default an agent sees its own copy and any shared services, but not other agents' copies (my call).

Which checkout a tool reads (my call):
- **The problem.** A plugin tool call carries no caller identity, and `fs` paths are relative to the project root. An agent works in its own worktree, inside the project at `.jensen/worktrees/<name>`.
- **The fix.** Every tool takes an optional `checkout` argument, the agent's worktree path relative to the project root, and the file tools also take an optional `files` list. The tool descriptions tell agents to pass their checkout, and without one the tool reads the primary checkout. That way a readiness fix the agent makes on its branch clears the finding it sees.
- **Later.** Once Jensen passes the caller's session and checkout to backend calls (R3), the argument becomes a fallback.
- **Verify first.** Check that the plugin's `fs` scopes reach files under `.jensen/worktrees/`. If they don't, that becomes a request.

1. **diagnose** (the main tool). One call answers "what's wrong with my copy":
   - crash loops, out-of-memory kills, failing health checks and port clashes;
   - the top errors with counts and an example each;
   - stack-trace paths inside containers mapped to project files through the bind mounts;
   - when the first failure happened, and which commits on the session's branch came just before it;
   - recent events, with `since` for cheap polling;
   - a suggested next step, including any readiness problem that explains the failure.
2. **wait** blocks until a service is healthy, a log line appears, or a timeout expires. It returns the outcome, plus a short diagnosis on failure. It replaces sleep-and-check loops, and needs a wait primitive in the backend (R3).
3. **logs** searches one service's logs by time, level or pattern, with the compaction above and a cursor.
4. **topology** describes the infrastructure in a few lines: what the files declare, how services link, ports and volumes, what runs in the agent's own copy, which code service each container builds from, and which services are shared.
5. **readiness** returns the 6.4 findings with the exact edits.

Start, stop and exec stay with Run's `run` tool. Removing the agent's own stack belongs there too (6.5). If Run doesn't add it, a sixth tool, `cleanup`, limited to the caller's session, covers it.

### 6.7 Tying what runs to a copy

What runs is matched to a copy in this order:
1. **Run's ownership**: which session started it (R1).
2. **The Compose project name `jensen-<session>`** that Run's sandbox uses. It names the session even before R1, once the plugin can read the sandbox (R2).
3. **Compose's `project.working_dir` label**: a path under a worktree means that worktree's copy, and a path in the primary checkout means You.
4. **Bind-mount sources** inside a worktree or the primary checkout.

A worktree maps to its session and plan through the branch recorded in plan files, or directly through R1.

A **leftover** is anything tied to the project whose worktree or session no longer exists.

The case where several worktrees share one Compose project name is detected and shown, not hidden. horus is an example (6.8).

### 6.8 horus as the first test

horus isn't a target, but it exercises the hard cases well:
- **Its Compose file isn't at the root.** `make up` runs `docker compose --env-file infra/versions.env -f infra/local/docker-compose.yml`, so the file lives in a subfolder.
- **Every checkout gets the same project name.** Compose names the project `local`, after the folder, so every copy started by hand from any worktree is the same project, with the same `local-db-1` container and volume.
- **The host port is fixed.** `127.0.0.1:5432:5432` is a readiness case, and the Makefile also hardcodes `PGPORT=5432` for some targets.
- **Two services aren't containers.** The panel (`go run ./cmd/panel`, port 8080) and horus-mcp (`uv run horus-mcp`, port 8092) are host processes started from the Makefile.
- **The sandbox isn't installed** on the owner's machine, so agents' runs of this project will need `jensen sandbox setup` first.
- **Under Run, the image probably won't build.** Run passes no `--env-file`, and there's no `.env` in `infra/local`. The Dockerfile's `ARG PG_MAJOR` and `ARG DEBIAN_RELEASE` have no defaults, so `FROM postgres:${PG_MAJOR}-${DEBIAN_RELEASE}` would come out empty. That makes it the first real case for the "variables nothing provides" rule.
- **Every session would bind port 5432.** Under Run, each session gets its own project (`jensen-<session>`), but every one binds `127.0.0.1:5432`, and on Linux so does the owner's own `make up`.

## 7. Architecture

- **The bundle.** One `main.js` bundle. The headless copy registers the pane, commands, menus, the status bar item and the settings tab. It holds state, watches files and feeds the status bar. The custom pane copy draws the rail, the map and the panel.
- **The map in the bundle.** The graph library and layout are the builder's call. The bundle also loads in the headless realm, so keep the map code out of the headless path where possible and watch the bundle size.
- **The Rust backend**:
  - Compose loading: merging several files, `include`, `extends`, profiles and env interpolation, as far as can be done from the files. Anything unresolved is reported as unresolved, never guessed silently.
  - Readiness rules, log compaction, diagnosis, and the five agent tools.
- **Data in the first version**: Compose files read through `fs` (UI) and the backend (agent tools), plus the code map through `graph`. Step 1 must not depend on any new host import. A backend that imports a function Jensen lacks would most likely fail to load, and the UI's parsing runs through the same backend.
  - In step 1 the agent tools look for the usual Compose file names at the checkout root (`compose.yaml`, `compose.yml`, `docker-compose.yaml`, `docker-compose.yml`), follow `include:`, and read any paths passed in `files`.
  - Agents can find Compose files with their own file tools and pass them in.
  - The extra paths the person sets in the plugin's settings aren't visible to the backend, which can't read plugin storage, until R3 lands.
- **Data in the second version**: R1 to R4.
- **Permissions in the first version**: `workspace`, `editor: "write"` (to apply accepted fixes as undoable edits), `graph`, `backend`, and `fs` scopes for YAML files (`*.yaml`, `*.yml`).
  - Dockerfiles often have no extension, so reading them anywhere would need the whole project as a scope. Until R7, the first version reads Dockerfiles only where a scope allows, and otherwise uses only the build context path from the Compose file (my call).
  - **YAML only, by choice.** Asking for env files, or for the whole project, would put secrets within reach and make the consent screen the most invasive one possible. That conflicts with the owner's anti-goal (my call).
  - **Verify first.** Check whether a file-type scope lets `fs.list` walk folders and `fs.watch` watch the project. If it doesn't, the step 1 fallback is the usual file names at the root plus the paths set in settings, rescanned when a YAML file is saved (`editor.saved`) and when the pane opens. Listing and watching under file-type scopes then joins R7. There is no polling.
- **Tests**:
  - the SDK's `TestHost` for the plugin;
  - `cargo test` for the backend;
  - fixture projects covering:
    - a Compose file at the root;
    - one in a subfolder;
    - several files;
    - profiles;
    - env files;
    - the app on the host with only the database in a container;
    - `container_name`;
    - external volumes;
    - a horus-like layout.

## 8. Settings tab

My call, kept small:
- a switch per kind of alert;
- extra Compose file paths the plugin should read. Plugin storage is shared across all of a user's projects, so until R8 these paths apply in every project where they exist;
- whether to show non-container services;
- how often the leftovers toast may fire.

## 9. Requests to Jensen

These are written to be general, so other plugins can use them too. Each request lists what it unlocks.

Where each request lands:
- **Jensen's app** owns the protocol (`schema/plugin-api.v1.json`), enforces permissions and the consent screen, and implements every capability. Its source isn't published, so this side is the IDE developer's.
- **The SDK** then follows: it vendors the new protocol and regenerates its types, adds TypeScript wrappers, adds functions to the Rust backend crate (`jensen-plugin-backend`), updates the manifest schema for new permissions, and extends `TestHost`. The SDK is public and MIT licensed, so the owner can prepare this side.
- **R5 is Jensen only.**
- Each request that lands raises the plugin's `minAppVersion`.

- **R0. Let the backend list a directory** (an `fs_list` host import). Without it, the agent tools can't find Compose files outside fixed paths, and the plugin may not write an index into the repo. It's small. Step 1 works around it (section 7), and step 2 adopts it, raising `minAppVersion`.
- **R1. Read Run and session state.**
  - What it covers:
    - Run's detected services, including non-container ones;
    - runs, each with its owning session, worktree, branch and plan;
    - shifted ports and status;
    - the sessions with their Jensen status;
    - the worktrees.
  - Events when any of this changes, including when a session or worktree goes away.
  - It unlocks the rail, non-container services on the map, ownership, the alert rule (which needs session status), and leftovers.
- **R2. Read the project's container state, from both Docker and Jensen's sandbox.**
  - Containers: labels, state, health, restarts, ports, mounts, image.
  - Volumes and images with sizes, and networks.
  - Logs, by time range or tail, streamed or paged.
  - Events.
  - It unlocks live state, details, logs, cleanup and diagnosis.
- **R3. The same access from inside the backend**, plus:
  - git history;
  - a wait primitive (block until a condition holds or a timeout expires), since the backend has no clock;
  - the caller's identity (session and checkout) on every tool call;
  - read access to the plugin's own settings or storage.

  It unlocks diagnose, wait and logs as agent tools, the "own copy" rule, and the person's extra Compose paths in agent tools.
- **R4. Actions through Jensen.**
  - Ask Run to start, stop or restart a service in a copy.
  - Stop or restart a container started outside Jensen.
  - Remove leftovers.
  - Each of these comes after the person confirms.
  - Also, from the backend, remove the caller's own session stack. That needs no confirmation, since it's limited to the caller's session by R3's caller identity.
  - It unlocks node actions, the person's cleanup, and the fallback `cleanup` agent tool.
- **R5. Two Run features.**
  - Sharing per service and per task: project defaults plus a per-task override when an agent starts its stack (decision 4).
  - Passing each session's shifted ports to the project, for example as environment variables, under a stable naming convention the readiness fixes can target. As relayed, Compose ports aren't shifted at all today. This makes the request essential for any stack with a fixed host port, and the first step is to confirm it.
  - Letting a project tell Run which env file its stack needs, for example through `servers.yaml`, since Run passes no `--env-file`.
  - A remove action in the `run` tool, so an agent can remove its own session's stack: containers, network, volumes and images under `jensen-<session>`.
  - Removing a session's Compose project, volumes and images included, whenever Jensen removes that session's worktree, and keeping it as long as the worktree is kept.
- **R6. UI additions:**
  - toasts with an action;
  - a count badge on a pane's tab;
  - a chip on an agent's session card;
  - opening a session pane;
  - handing context to a session ("Ask the agent");
  - decorations with a fix action.
- **R7. Narrower file permissions**:
  - read-only scopes;
  - filename scopes such as `Dockerfile*` and `.env*`;
  - listing and watching folders under file-type scopes, if that isn't already allowed.
- **R8. Project identity.** Tell the plugin which project it runs in, for example with a project id in the startup handshake, or keep plugin storage per project. The developer confirmed that storage is shared across all of a user's projects, so this is needed for any per-project state: extra Compose paths, seen markers, and when the leftovers toast last fired.

## 10. Delivery

### Step 1: needs no request

- The map from Compose files, files only: services, links, volumes, ports, and the code-map link.
- Readiness: editor marks, hover text, the fix from the right-click menu, and a readiness list in the pane.
- Navigation: map node to file and line, "Show in map" from the editor, and the explorer menu.
- A status bar item with the readiness count.
- The `topology` agent tool (files only) and the `readiness` agent tool.
- The settings tab.

Step 1 goes to the store and gets tested on horus while the requests are discussed.

### Step 2: after R1 to R4

- Live state for every copy.
- The rail of copies, with problems first and unseen markers.
- Non-container services from Run.
- The details panel with its links.
- The alerts.
- The diagnose, wait and logs tools.
- Cleanup: the person's review of leftovers, and agents removing their own copy, through Run's remove action or the fallback `cleanup` tool.
- Node actions through Run.

Jensen removing a session's stack together with its worktree arrives whenever that part of R5 lands. The plugin doesn't wait for it.

A "needs recreate" cue, which compares Compose's `config-hash` label with the files, belongs here if it comes cheap (my call).

### Later, on demand

- R5 sharing, once Run supports it, and R6 polish.
- Other orchestrators.
- Notes blocks.
- The endpoint probe.
- Incident memory.
- Resource figures on the map.
- The event timeline.
- The network inspector.
- An effective config view, which needs secret masking.
- Cross-project port conflicts.

## 11. The original feature list

1. Declared topology: kept, limited to Compose and the Dockerfiles it builds. Step 1.
2. Runtime discovery: changed. It goes through Jensen (R2), covering Docker and the sandbox, with no own adapters.
3. Project scoping: changed. Run's ownership first, then Compose labels and bind mounts (6.7).
4. Shared model: kept. Nodes are services (container or process), volumes, networks and ports. Edges are depends-on, network, publishes, mounts, and connects (inferred and labelled).
5. Graph pane: kept, as the custom pane in 6.1.
6. Declared vs running overlay: kept, as map styling per selected copy.
7. Node actions: changed. They go through Run or Jensen with confirmation (R4). No exec.
8. Details panel: kept. Step 2.
9. Resource overlay: later.
10. Status bar summary: kept. Readiness in step 1, live state in step 2.
11. Log pane: changed. Errors appear in the details panel and Run's drawer keeps live logs. A full log pane comes later.
12. Event timeline: later.
13. Live status in the editor: kept. Readiness in step 1, live hints in step 2.
14. Drift detection: step 2 if cheap, via `config-hash`.
15. Two-way navigation: kept. Step 1.
16. Log errors to source: kept, inside diagnose and the details panel. Step 2, because it needs mounts.
17. Code-graph link: kept. Step 1.
18. Explorer menu actions: changed to "Show in Containers" and "Check readiness". Up and Down stay with Run.
19. `containers_overview`: folded into `topology`, plus Run's status.
20. `container_logs`: kept, as `logs`.
21. `container_inspect`: folded into `diagnose` and `topology`.
22. `container_events`: folded into `diagnose` (`since`).
23. `container_action`: cut, because Run owns lifecycle.
24. `containers_diagnose`: kept, as `diagnose`, the main tool.
25. `container_wait`: kept, as `wait` (needs R3).
26. `endpoint_probe`: later. Agents can already curl a port.
27. Git correlation: folded into `diagnose` (needs R3).
28. Incident memory: later. Jensen's memory covers it for now.
29. Port map: changed. Ports per copy are on the map, and fixed ports are a readiness rule. Cross-project conflicts come later.
30. Network inspector: later. The map shows networks.
31. Effective config view: later.
32. Saved profiles: changed into Run sharing and profiles (R5). Later.
33. Cleanup with a dry run: kept, with three owners (6.5). Agents remove their own copy, Jensen removes a session's stack with its worktree, and the person reviews the rest with sizes first. Step 2, plus R5 for the Run parts.
34. Volume snapshots: later, possibly cut.
35. Topology in notes: later.

New features, not on the original list:
- copies by session and worktree;
- readiness with suggested edits;
- the alert rule;
- non-container services from Run;
- the leftovers definition;
- the `topology` and `readiness` agent tools.

## 12. Left to the builder

- The graph library, the layout, and the visual styling, using Jensen's theme tokens.
- How deep Compose loading goes. It's best effort, and anything unresolved is labelled.
- The compaction details: line normalisation, error templating, stack-trace folding per language.
- Alert thresholds, and the heuristics behind inferred links, which must be labelled.
- The internal state model and how the headless copy and the pane copy share it.
- The plugin's store id and display name, unless the owner sets them.

## 13. Questions for the IDE developer

Answered on 2026-10-19:
1. **How Run runs a Compose service.** It runs `nerdctl compose -f <file> -p jensen-<session> up -d --build <service>`, directly on the host on Linux and through a VM on macOS. Sections 3, 6.4, 6.7 and 6.8 reflect this.
2. **Plugin storage** is shared across all of a user's projects. That makes R8 a requirement for per-project state.

Still open:
- Does Run pass anything else to a Compose stack, such as environment variables for the session's port band? The relayed command suggests not, which makes R5 essential.

The brainstorm mockups are in `docs/specs/2026-10-19-containers-mockups/`. They are HTML fragments written for the brainstorm's preview frame, so opened on their own they render with rough styling.
- `layout.html`: the three layouts.
- `layout-v2.html`: the two combined layouts.
- `attention.html`: the chosen layout with the alerts. Its port tooltip is superseded by 6.4.
