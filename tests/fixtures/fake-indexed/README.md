# fake `indexed` scripts

Stand-ins for the external `indexed` binary, installed onto a `VaultFixture`'s private `bin/`
directory (which the fixture already prepends to `PATH`) by `install_fake` in
`tests/search_cli.rs`.

`VaultFixture::fake_indexed(payload)` covers the happy path — it writes a script that records
its argv and echoes a fixed `--simple-output` envelope payload. These two cover the paths it
cannot:

| script | behaviour | what it exercises |
|---|---|---|
| `fail.sh` | prints nothing, exits 3 | a non-zero exit degrades to the built-in engine |
| `hang.sh` | sleeps 60 s, never writes | the 30 s wall clock (deviation 12) |
| `no-collection.sh` | `index update <C>` prints the shipped CLI's not-found error and exits 1; every other command exits 0 | `mesh reindex`'s update-when-exists routing (`index update` first, `index create files …` only when the update reports the collection absent) |

**`$MESH_INDEXED_TIMEOUT_MS`** overrides the wall clock, in milliseconds. It exists so the
timeout test finishes in a fraction of a second instead of half a minute; a value of `0` or an
unparsable one falls back to the built-in default. It is read by
`mesh::search::indexed::timeout()` for search (30 000 ms) and by
`mesh::search::indexed::ingest_timeout()` for the create/update ingest path (600 000 ms).
