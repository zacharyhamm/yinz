---
name: yinz-plugin-data
description: Discover Yinz plugin schemas, query and paginate shared or private records, and apply guarded atomic data mutations through a running session.
---

Use `yinz --session NAME data ...`. Access is served by that session's configured
PluginDataStore, including Postgres when configured. It does not open an offline
database. Prefer project, note, todo, and agent domain commands for their records.
Use generic data operations when those commands do not cover the requested task.

Start with `yinz --session main data catalog`. It returns explicit namespaces,
entity definitions, and owning plugins. Schema creation and migration belong to
those plugins. CLI catalog access includes private namespaces; this does not
change plugin permissions. Queries and writes require `--namespace`.

A fetch file is the existing FetchRequest JSON. For example, after discovering
the `project` entity in `shared:hocus`:

```sh
yinz --session main data fetch --namespace shared:hocus --file - <<'JSON'
{"entity":"project","filter":null,"search":null,"sort":[],"select":["id","uid","rev","name","root"],"limit":50,"offset":0,"after_id":null}
JSON
```

`data count` accepts the same request. Results contain columns and rows of tagged
values, such as `{"Text":"work"}` and `{"Int":1}`. Select IDs and revisions when
preparing edits. With default ID ordering, pass `next_cursor` back as `after_id`.
Honor `truncated`; sorted queries use offsets instead of keyset cursors. Filters
are not silently scoped to the selected host/session. Include both `host_id` and
`yinz_session_name` when querying session-owned operational rows.

`data apply --namespace N --file PATH` (or `-` for stdin) accepts a Mutation JSON
array. A replacement requires the revision read with the original value. For a
new stable uid, `expect_rev:0` asserts absence. Example shape:

```json
[{"Update":{"entity":"ENTITY","filter":{"Eq":["id",{"Int":42}]},"values":[["FIELD",{"Text":"new value"}]],"expect_rev":7}}]
```

Use actual entity/field names from the catalog and the row's actual revision.
Deletion requires an immediately preceding AssertRows with the same entity and
filter. Its rows are the complete matching set of `[id,rev]` pairs. Both
assertion and deletion run in one atomic batch; a mismatch rolls everything back.

One JSON result is returned on success, including the apply report. Failures are
nonzero with diagnostics on stderr. After conflict, reconcile with a fresh read.
After timeout or disconnection, read before repeating a mutation: it may have
committed. Verify by fetching the affected rows. Writes notify watches in the
selected server; another server's Postgres writes do not wake local watches.
