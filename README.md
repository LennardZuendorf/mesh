# shards

A mesh for multi-agent collaboration over a single Tolaria Markdown folder. Three verbs — `note`, `task`, `search` — give a fleet of agents and their human operator a shared substrate through low-level tools: a CLI and an MCP server.

- Notes + search = shared memory.
- Tasks = coordination + handoff (`claim` / `finish` / `cancel`; dependency graph deferred).
- Markdown is the source of truth; shards owns the interface (and writes), not the data.

Search delegates to the first-party [`indexed`](https://github.com/LennardZuendorf/indexed) engine; shards coexists with the Tolaria vault MCP on one folder — no database to run.

The spec is the source of truth: see [`.spec/`](.spec/). Working in here? Read [`AGENTS.md`](AGENTS.md) first.
