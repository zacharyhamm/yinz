// A unit-host reply fixture. Integration cases use the real disposable SQLite database.
export function storeReplies() {
  const rows = new Map<string, { value: string; rev: number }>();
  const columns = ["uid", "value", "rev"];
  return {
    rows,
    replies: {
      DataDefineEntity: null,
      DataFetch: ({ request }: { request: { filter?: { Eq?: [string, { Text: string }] } } }) => {
        const uid = request.filter?.Eq?.[1].Text;
        return {
          columns,
          rows: [...rows]
            .filter(([key]) => uid === undefined || uid === key)
            .map(([key, row]) => [{ Text: key }, { Text: row.value }, { Int: row.rev }]),
          truncated: false,
          next_cursor: null,
        };
      },
      DataApply: ({
        mutations,
      }: {
        mutations: {
          Put: { uid: string; values: [string, { Text: string }][]; expect_rev: number };
        }[];
      }) => {
        for (const { Put: p } of mutations) {
          if ((rows.get(p.uid)?.rev ?? 0) !== p.expect_rev) throw new Error("Conflict");
          rows.set(p.uid, {
            value: p.values.find(([name]) => name === "value")![1].Text,
            rev: p.expect_rev + 1,
          });
        }
        return { per_mutation: mutations.map(() => 1) };
      },
    },
  };
}
