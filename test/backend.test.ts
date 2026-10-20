import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { start, TestHost } from "@jensen-org/plugin-sdk";
import { type AnalyzeResult, analyze } from "../src/backend";
import ContainersPlugin from "../src/main";

const sample = JSON.parse(
  readFileSync(new URL("./fixtures/analyze-sample.json", import.meta.url), "utf8"),
);

test("the mirror matches the JSON the backend writes", () => {
  const mirror = {
    model: {
      projectName: { value: "local", origin: "folder" },
      files: ["infra/local/docker-compose.yml"],
      services: [
        {
          name: "db",
          image: "postgres:16",
          build: { context: "infra/local", dockerfile: "Dockerfile.db" },
          ports: [
            {
              hostIp: "127.0.0.1",
              host: "5432",
              container: "5432",
              protocol: "tcp",
              fixed: true,
              source: { file: "infra/local/docker-compose.yml", line: 20, column: 8 },
              span: { from: 412, to: 429 },
            },
          ],
          volumes: [
            { kind: "named", source: "data", target: "/var/lib/postgresql/data", readOnly: false },
            {
              kind: "bind",
              source: "./init",
              target: "/docker-entrypoint-initdb.d",
              readOnly: true,
            },
          ],
          networks: ["backnet"],
          dependsOn: [],
          containerName: "app-db",
          profiles: ["dev"],
          source: { file: "infra/local/docker-compose.yml", line: 12, column: 2 },
        },
        {
          name: "app",
          ports: [],
          volumes: [],
          networks: [],
          dependsOn: ["db"],
          profiles: [],
          source: { file: "infra/local/docker-compose.yml", line: 30, column: 2 },
        },
      ],
      volumes: [
        {
          name: "data",
          fixedName: "app-data",
          external: false,
          source: { file: "infra/local/docker-compose.yml", line: 40, column: 2 },
        },
      ],
      networks: [
        {
          name: "backnet",
          external: true,
          source: { file: "infra/local/docker-compose.yml", line: 44, column: 2 },
        },
      ],
      edges: [
        { from: "app", to: "db", kind: "depends_on" },
        { from: "db", to: "backnet", kind: "network" },
        { from: "db", to: "data", kind: "volume" },
        { from: "app", to: "db", kind: "links" },
      ],
    },
    findings: [
      {
        rule: "fixed_host_port",
        service: "db",
        message: "db publishes host port 5432, so a second copy of this project can't start.",
        affects: ["agents", "by_hand"],
        lead: false,
        source: { file: "infra/local/docker-compose.yml", line: 20, column: 8 },
        span: { from: 412, to: 429 },
        edit: {
          file: "infra/local/docker-compose.yml",
          span: { from: 412, to: 429 },
          newText: `"127.0.0.1:\${DB_PORT:-5432}:5432"`,
          title: "Make db's host port configurable",
        },
      },
    ],
    unresolved: [
      {
        variable: "PG_MAJOR",
        source: { file: "infra/local/docker-compose.yml", line: 14, column: 20 },
      },
    ],
    errors: [
      {
        file: "infra/local/docker-compose.yml",
        message: "can't include extra.yml: no such file",
        source: { file: "infra/local/docker-compose.yml", line: 3, column: 4 },
      },
    ],
  } satisfies AnalyzeResult;

  expect(mirror).toEqual(sample);
  // An error's source, when it has one, is in the file the error is reported in.
  for (const error of mirror.errors) expect(error.source.file).toBe(error.file);
});

test("analyze calls the backend's analyze method", async () => {
  const host = new TestHost();
  host.respondTo("backend.call", () => ({ result: sample, touched: [] }));
  let result: AnalyzeResult | undefined;
  await start(
    class extends ContainersPlugin {
      override async onload(): Promise<void> {
        result = await analyze(this.app, {
          files: ["compose.yaml"],
          checkout: ".jensen/worktrees/a",
        });
      }
    },
    { connection: host.connection },
  );
  await host.settle();

  expect(host.callsTo("backend.call")[0]).toEqual({
    method: "analyze",
    input: { files: ["compose.yaml"], checkout: ".jensen/worktrees/a" },
  });
  expect(result).toEqual(sample);
  host.close();
});
