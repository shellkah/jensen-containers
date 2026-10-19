import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";

const allowed = ["jensen.fs_read", "jensen.log"];

test("backend.wasm imports only Jensen's read and log functions", () => {
  const wasm = new WebAssembly.Module(readFileSync(new URL("../backend.wasm", import.meta.url)));
  const imported = WebAssembly.Module.imports(wasm).map((entry) => `${entry.module}.${entry.name}`);

  // An fs_write import would mean something writes into the project: hands off the project.
  expect(imported.filter((name) => !allowed.includes(name))).toEqual([]);
});
