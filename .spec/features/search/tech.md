---
type: feature-tech
feature: search
sibling: product.md
parent: ../../tech.md
updated: 2026-06-21
---

# Search — Tech

Thin wrapper over `indexed`. Corpus = all `*.md` under `notes/` + `tasks/` (broader than `note list`).

**Links:** [product](product.md) · [plan](plan.md)

## Files

`cli/search.py` · `index/indexed_client.py` · `tagpull.py` · `fallback.py`

## API

`search "<q>" [--type] [--tags] [--owner] [--status] [--limit 10] [--threshold 0.65] [--meta-only] [--full]` — query optional with `--tags`. JSON default.

**Result shape:** `{id, type, title, score, tags?, owner?, updated?, snippet?, path}`

## indexed contract

```bash
indexed index search "<q>" --collection <vault> --json --limit N
# hit: {"path":"…","score":0.91,"snippet":"…"}
indexed index update <path> --collection <vault>
indexed index create <tolaria_path> --collection <vault>   # reindex
```

Map `path` → frontmatter via warm index. Recency tiebreak if scores within `0.02`.

## Degradation

| Condition | Mode |
|---|---|
| daemon up + indexed + hybrid=true | hybrid |
| daemon down / no indexed / hybrid=false | substring |

Substring scores: title exact 1.0 · substring 0.8 · tag 0.6 · body 0.4; sort score desc, `updated` desc.

`indexed_client`: `incremental_update`, `full_rebuild`; registers on watcher hook.
