# Containers

See a project's container stack from its Compose files: a map, readiness checks with suggested edits, and short answers for agents.

## What it does

It works from the Compose files alone. It starts and stops nothing, uses no network and runs nothing in the background.

- **A files-only map.** The project's services, the ports they publish, the volumes and networks they use and what depends on what, read from the Compose files. It shows what the files declare, not what is running.
- **Readiness checks with suggested edits.** Checks for what stops a stack from running twice on one machine, such as a fixed host port or a fixed container name. A finding that a file edit can fix comes with that edit, and nothing changes until you accept it.
- **Agent tools.** The same reading and checks answer agents in short plain text, so an agent can learn the shape of a stack and what is not ready without reading every file.

## What it asks for

| Permission | Why |
|---|---|
| `workspace` | for its pane |
| `editor: "write"` | to apply a fix only after you accept it, as one undoable edit |
| `graph` | to link a service to the code it builds from |
| `backend` | to parse Compose files and run the checks in a sandboxed WebAssembly program that agents call too |
| `fs` on `*.yaml` and `*.yml` | to read Compose files. It never reads `.env` files. The scope also allows writing, but the plugin never writes through it: a fix you accept is applied through the editor, as one undoable change |

It asks for nothing else, and Jensen asks you before granting any of these.

## Develop

You need [bun](https://bun.sh) and a Rust toolchain from [rustup](https://rustup.rs).

```bash
make check   # lint, typecheck, Rust tests, build, TypeScript tests, validate
make build   # the WebAssembly backend and main.js
```

`make build` adds the `wasm32-unknown-unknown` target to the Rust toolchain if it is missing, and a fresh checkout installs its own dependencies.

## Agent tools

Agents see two tools, named after the plugin's id:

- `dev_jensen_containers_topology` describes the stack in a few lines: services, ports, volumes, networks, links and the variables with no default.
- `dev_jensen_containers_readiness` lists what stops copies of the stack from running side by side, each with the exact edit that fixes it when an edit can.

Both take the same two optional arguments:

- `checkout`: the agent's worktree, relative to the project root, such as `.jensen/worktrees/<name>`. Without it, the primary checkout is read. An agent working in a worktree should pass it, so a fix made on its branch clears its finding.
- `files`: Compose files relative to the checkout root, such as `infra/local/docker-compose.yml`, the `-f` files of one Compose project, merged in order. Without it, the tools read the first of `compose.yaml`, `compose.yml`, `docker-compose.yaml` and `docker-compose.yml` at the checkout root, and say so when there is none.

Both tools only read files. They write nothing, start nothing and show what the files declare, not what is running.

### Known limitations

- `!reset` and `!override` tags are ignored, so an override that resets a base's ports still shows the base's port.
- A `container_name` or `name:` overridden in a later `-f` file is reported there. Removing it reveals the earlier one on the next call.
- With no `files`, only the main Compose file is read, not `compose.override.yaml`.
