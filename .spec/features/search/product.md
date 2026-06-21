---
type: feature-product
feature: search
sibling: tech.md
parent: ../../product.md
updated: 2026-06-21
---

# Search — Product

`search` = recall. Wraps [`indexed`](https://github.com/LennardZuendorf/indexed) (hybrid rank) + tag-pull + substring fallback.

**Links:** [product](../../product.md) · [tech](tech.md) · [plan](plan.md)

## Scope

| | |
|---|---|
| **Owns** | `brain search`, indexed wrapper, tag-pull, fallback, `indexed_client` |
| **Does not own** | ranking engine (`indexed`), watcher (daemon) |

## Requirements

### R1: Hybrid
SHALL search all `notes/` + `tasks/` markdown (incl. non-brain files); JSON hits with `id`, `type`, `title`, `score`, `path` (`id: null` if foreign).

### R2: Tag pull
SHALL `--tags` without query — frontmatter only, zero embed cost.

### R3: Output modes
SHALL `--meta-only`, `--full`, `--threshold`.

### R4: Degrade
SHALL substring-scan per [tech](tech.md) matrix + stderr notice.

## UX

```bash
brain search "CLID decision"              # hybrid when available
brain search --tags ndc --meta-only       # tag pull
brain search "ndc"                        # degraded path
```

**Not:** vectors in Brain, synthesis/think layer.
