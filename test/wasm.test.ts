import { expect, test } from "bun:test";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { AnalyzeResult } from "../src/backend";
import { loadBackend } from "./support/wasm-host";

const wasmPath = new URL("../backend.wasm", import.meta.url).pathname;
if (!existsSync(wasmPath)) throw new Error("build backend.wasm first: make build");

const allowed = ["jensen.fs_read", "jensen.log"];

test("backend.wasm imports only Jensen's read and log functions", () => {
  const wasm = new WebAssembly.Module(readFileSync(wasmPath));
  const imported = WebAssembly.Module.imports(wasm).map((entry) => `${entry.module}.${entry.name}`);

  // An fs_write import would mean something writes into the project: hands off the project.
  expect(imported.filter((name) => !allowed.includes(name))).toEqual([]);
  // analyze reads the Compose files, so fs_read must be there.
  expect(imported).toContain("jensen.fs_read");
});

test("analyze runs in backend.wasm on the horus-like layout", async () => {
  const backend = await loadBackend("backend.wasm", "fixtures/horus-like");
  const { ok } = backend.call("analyze", { files: ["infra/local/docker-compose.yml"] }) as {
    ok: AnalyzeResult;
  };
  expect(ok.model.projectName).toEqual({ value: "local", origin: "folder" });
  expect(ok.model.services[0]?.ports[0]).toMatchObject({
    hostIp: "127.0.0.1",
    host: "5432",
    container: "5432",
    fixed: true,
    source: { file: "infra/local/docker-compose.yml", line: 20, column: 8 },
  });
  expect(ok.unresolved.map((u) => u.variable)).toEqual([
    "DEBIAN_RELEASE",
    "PG_MAJOR",
    "POSTGRESQL_VERSION",
    "TIMESCALEDB_VERSION",
    "PGVECTOR_VERSION",
  ]);
  expect(ok.findings.map((finding) => finding.rule)).toEqual([
    "fixed_host_port",
    "same_project_name",
    "unprovided_variable",
    "unprovided_variable",
    "unprovided_variable",
    "unprovided_variable",
    "unprovided_variable",
  ]);
  expect(backend.reads).toEqual(["infra/local/docker-compose.yml"]);
});

test("the tools run in backend.wasm on a worktree copy", async () => {
  const root = mkdtempSync(join(tmpdir(), "containers-"));
  try {
    const worktree = join(root, ".jensen", "worktrees", "feat-x");
    mkdirSync(worktree, { recursive: true });
    cpSync(new URL("../fixtures/horus-like", import.meta.url).pathname, worktree, {
      recursive: true,
    });
    const backend = await loadBackend("backend.wasm", root);
    const checkout = ".jensen/worktrees/feat-x";

    const readiness = backend.call("readiness", {
      checkout,
      files: ["infra/local/docker-compose.yml"],
    }) as { ok: { text: string } };
    const text = readiness.ok.text;
    expect(text.split("\n")[0]).toBe(
      "Readiness of infra/local/docker-compose.yml in .jensen/worktrees/feat-x: 3 problems stop copies of this stack from running side by side.",
    );
    expect(text).toContain(
      `Fix: replace "127.0.0.1:5432:5432" with "127.0.0.1:\${DB_PORT:-5432}:5432".`,
    );

    // The Compose file isn't at the checkout root, so the agent is told to pass files.
    const topology = backend.call("topology", { checkout }) as { ok: { text: string } };
    expect(topology.ok.text).toStartWith("No Compose file at the root of .jensen/worktrees/feat-x");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
