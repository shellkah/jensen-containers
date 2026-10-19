# Containers

See a project's container stack from its Compose files: a map, readiness checks with suggested edits, and short answers for agents.

## What it does

Step 1 works from the Compose files alone. It starts and stops nothing, uses no network and runs nothing in the background.

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
| `fs` on `*.yaml` and `*.yml` | to read Compose files. It never reads `.env` files, and it never writes one |

It asks for nothing else, and Jensen asks you before granting any of these.

## Develop

You need [bun](https://bun.sh) and a Rust toolchain from [rustup](https://rustup.rs).

```bash
make check   # lint, typecheck, Rust tests, build, TypeScript tests, validate
make build   # the WebAssembly backend and main.js
```

`make build` adds the `wasm32-unknown-unknown` target to the Rust toolchain if it is missing, and a fresh checkout installs its own dependencies.
