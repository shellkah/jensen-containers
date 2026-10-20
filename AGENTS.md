# Notes for agents

## Specs and plans

- Specs live in `docs/specs/`: one Markdown file per design, named `YYYY-MM-DD-<topic>-design.md`, with its mockups in a folder beside it. Put new specs there, never under `docs/superpowers/`.
- Plans are managed by Jensen only. Write a plan with Jensen's `plan_template` and `plan_register`, follow it with `plan_update`, and never commit a plan file to this repo. When a skill such as superpowers:writing-plans says to save a plan under `docs/`, register it with Jensen instead.
- A plan split into parts keeps its index (the parts and the interfaces between them) in its Jensen plans, not in a file. Each part is a Jensen plan of the same chain.
