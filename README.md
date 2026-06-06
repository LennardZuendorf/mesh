# 🧠 brain

A thin coordination layer over a single [Tolaria](https://github.com/LennardZuendorf) Markdown
folder. **Three verbs — `note`, `task`, `search`. One daemon. One folder. All agents.**

- **Notes + search = memory** — no separate memory store.
- **Tasks = coordination = handoff** — `claim` / `finish` / `blocks`.
- **Markdown is the source of truth** — brain owns the interface, not the data.

```bash
brain note new "Use CLID fallback for Lufthansa GDS" --type decision --tags ndc
brain task new "Audit NDC fallback logic" --owner flights-agent
brain task ready --owner flights-agent      # what's unblocked & unclaimed?
brain task claim t-c7d1 && brain task finish t-c7d1 --outcome "Resolved via CLID."
brain search "how did we handle the CLID fallback decision"
```

CLI-first (MCP over the same daemon); hybrid search via `indexed.sh`; identity via `$BRAIN_AGENT`.

## Status

📋 **Spec stage** — no implementation yet. This is a **spec-driven** repo.

- **The spec is the source of truth:** [`spec/README.md`](spec/README.md)
- **Working in this repo?** Read [`AGENTS.md`](AGENTS.md) first.

## License

TBD — a permissive license (MIT or Apache-2.0) is recommended.
