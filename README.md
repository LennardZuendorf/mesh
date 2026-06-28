# brain

A thin coordination layer over a single Tolaria Markdown folder. Three verbs — `note`, `task`, `search` — one daemon, one folder, shared by a human and a fleet of agents.

- Notes + search = memory.
- Tasks = coordination + handoff (`claim` / `finish` / `cancel`; dependency graph deferred).
- Markdown is the source of truth; brain owns the interface (and writes), not the data.

Search delegates to the first-party [`indexed`](https://github.com/LennardZuendorf/indexed) engine; brain coexists with the Tolaria vault MCP on one folder — no database to run.

Spec stage — no code yet. The spec is the source of truth: see [`.spec/`](.spec/). Working in here? Read [`AGENTS.md`](AGENTS.md) first.
