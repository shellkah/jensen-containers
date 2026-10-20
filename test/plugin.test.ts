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

test("declares exactly the topology and readiness tools", () => {
  const tools =
    (pkg.jensen as { tools?: { name: string; description: string; inputSchema: unknown }[] })
      .tools ?? [];
  expect(tools.map((tool) => tool.name)).toEqual(["topology", "readiness"]);
  const schema = {
    type: "object",
    properties: {
      checkout: {
        type: "string",
        description:
          "Another checkout of this project to read, relative to the project root, such as .jensen/worktrees/<name>. Leave it out to read your own checkout: Jensen reads the files of the session that calls the tool.",
      },
      files: {
        type: "array",
        items: { type: "string" },
        description:
          "Compose files relative to the checkout root, such as infra/local/docker-compose.yml: the -f files of one Compose project, merged in order. Leave it out to use compose.yaml, compose.yml, docker-compose.yaml or docker-compose.yml at the checkout root.",
      },
    },
    additionalProperties: false,
  };
  for (const tool of tools) expect(tool.inputSchema).toEqual(schema);
  expect(tools[0]?.description).toBe(
    "Describe this project's container stack in a few lines, from its Compose files: services, ports, volumes, networks, links and variables with no default. It reads your own checkout; pass files when the Compose files aren't at the checkout root. It reads files only and doesn't show what is running.",
  );
  expect(tools[1]?.description).toBe(
    "List what stops copies of this project's stack from running side by side: fixed host ports, container_name, volumes or networks with fixed names, one project name for every checkout, and variables nothing provides when Run starts the stack, each with the exact edit that fixes it when an edit can. It reads your own checkout, so a fix you make on your branch clears its finding; pass files when the Compose files aren't at the checkout root.",
  );
  expect(validatePackage(pkg)).toEqual([]);
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
