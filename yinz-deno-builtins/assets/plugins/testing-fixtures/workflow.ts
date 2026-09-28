import {session} from 'yinz:testing';
// The real bridge still dispatches Swamp through its normal host command. Only the
// external executable fails deterministically, so no installed Swamp or credentials are used.
export function failedWorkflow() {
  return session.fixture({'.yinz/swamp-workflows.kdl': 'agent_exit { workflow "fixture-exit"; }'}, {
    git: `if [ "$1" = "-C" ]; then printf 'worktree %s\\000' "$2"; else exit 1; fi`,
    swamp: 'echo fixture-swamp-unavailable >&2; exit 1',
  });
}
export function deliveryValues() {
  const data = session.data('yinz:swamp-bridge', 'delivery');
  const column = data.columns.indexOf('value');
  return data.rows.map(row => JSON.parse((row[column] as {Text:string}).Text));
}
