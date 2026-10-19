# Handover: Containers plugin, step 1

Spec: `docs/superpowers/specs/2026-10-19-containers-plugin-design.md`
Index: `docs/superpowers/plans/2026-10-19-containers-step-1-index.md`

## Write part 1 of 3: Core and agent tools

Run this in a fresh session on Opus, in `/home/fxxx/projects/jensen-containers`, once the first commit with `docs/superpowers/` exists.

```text
You are writing part 1 of 3 of a split implementation plan for the Jensen Containers plugin, in the repo /home/fxxx/projects/jensen-containers. You will not be able to talk to the person who asked for it.

Your inputs:
- Spec: docs/superpowers/specs/2026-10-19-containers-plugin-design.md
- Plan index: docs/superpowers/plans/2026-10-19-containers-step-1-index.md
- Your part: part 1 of 3, "Core and agent tools".
- Earlier parts: none. Code from earlier parts: none. The repo holds only docs/superpowers/.

Read the spec and the index. Your part's row in the index's Parts table says what your part delivers.

Read and follow superpowers:writing-plans for your part only. Do not split your part further, and do not write handover prompts.

This project uses Jensen, and Jensen's plan format replaces the writing-plans header and layout. Call plan_template, write the plan in that format, then register it as a draft with plan_register:
- id: containers-core-and-agent-tools
- risk: medium
- scope: the paths it touches

Map writing-plans onto that format like this:
- What I understood: the goal, the architecture, the tech stack, and the spec and index paths. Then the index's Global Constraints, copied verbatim. Then a Review Focus list: the five uncovered inputs or failure modes most likely to bite, one line each, and each one pinned by a test in the change that owns it.
- Changes: one "### N." item per task.
  - Fix: what to build, with exact files, names and signatures, the values the spec pins, and the task's Consumes and Produces interfaces.
  - Guard: the tests by name, with their assertions as code, and the TDD sequence: write the failing test, run it and see it fail, implement, run it and see it pass, then commit following Jensen's committing rules.
  - Files: the paths.
- Risks & mitigations: as the template says.
- Steps: one checkbox per task, naming the command whose success means it's done.
- Acceptance criteria: runnable commands, including make check.
- Verification: build the release, install it in Jensen, and call both agent tools against a horus-like layout.

Use the index's Cross-Part Interfaces exactly as written. Do not change them.

Facts you'll need:
- The SDK is at /home/fxxx/projects/jensen/plugin-sdk:
  - the scaffold template is in src/cli/create.ts (use it with --backend);
  - the protocol is protocol/plugin-api.v1.json;
  - the guides are in docs/guide;
  - the backend crate is crates/jensen-plugin-backend;
  - TestHost is in src/testing.ts.
- create refuses a non-empty folder, and the implementing checkout will already hold docs/.
- horus (/home/fxxx/projects/horus) is only a test case. Its Compose file is infra/local/docker-compose.yml, started with --env-file infra/versions.env. Build fixtures that mimic its layout, but never depend on horus.
- A hook blocks find, grep, rg, git grep, cat, head and ls -R in Bash. Use plain ls and the Read tool. Jensen's code tools can't see other repos.
- main serves an environment, so Jensen refuses agent writes to it. Write nothing into the repo: the plan goes only through plan_register. Approving the plan gives the implementing session its branch and worktree. If plan_register is refused, stop and say why.

Do not guess on a question that changes what gets built; report it instead. Decide everything else yourself, the way writing-plans says to. Once the plan is registered, run writing-plans' Self-Review on it, fix what you find, and register the fixed draft again under the same id.

When you are done, report exactly this:

Plan: <plan id>
Tasks: <count>
Interface differences from index: <none, or one line each: index says X, plan has Y>
Open questions: <none, or ranked, each with why the answer changes what gets built>
```
