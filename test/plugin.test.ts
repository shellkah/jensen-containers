import { expect, test } from "bun:test";
import { start, TestHost, validatePackage } from "@jensen-org/plugin-sdk";
import pkg from "../package.json";
import ContainersPlugin from "../src/main";

test("loads headless and reports ready", async () => {
  const host = new TestHost();
  await start(ContainersPlugin, { connection: host.connection });
  expect(host.callsTo("plugin.ready")).toHaveLength(1);
  host.close();
});

test("package.json passes the SDK's validator and asks for the index's permissions", () => {
  expect(validatePackage(pkg)).toEqual([]);
  expect(pkg.jensen.permissions).toEqual({
    workspace: true,
    editor: "write",
    graph: true,
    backend: true,
    fs: ["*.yaml", "*.yml"],
  });
  expect(pkg.jensen).toMatchObject({
    id: "dev.jensen.containers",
    displayName: "Containers",
    category: "devtools",
    minAppVersion: "0.3.0",
    backend: "backend.wasm",
  });
});
