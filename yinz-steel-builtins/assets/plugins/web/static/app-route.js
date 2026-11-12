// Hash routes (DOM-free):
//   #/                                         agents list
//   #/a/<encodeURIComponent(id)>?run=<run>&tab=timeline|screen|details[&bind=0]
// `bind=0` marks stored-run browsing (previous/original run links), which reads
// the run without the live agent binding. Routes never carry credentials.

export const TABS = ['timeline', 'screen', 'details'];

export function parseHash(hash) {
  const text = typeof hash === 'string' ? hash : '';
  const m = /^#\/a\/([^?]+)(?:\?(.*))?$/.exec(text);
  if (!m) return { view: 'list' };
  let id;
  try {
    id = decodeURIComponent(m[1]);
  } catch {
    return { view: 'list' };
  }
  if (!id.trim()) return { view: 'list' };
  const params = new URLSearchParams(m[2] || '');
  const tab = TABS.includes(params.get('tab')) ? params.get('tab') : null;
  return { view: 'detail', id, run: params.get('run') || '', tab, bind: params.get('bind') !== '0' };
}

export function detailHash({ id, run, tab, bind = true }) {
  const params = new URLSearchParams();
  if (run) params.set('run', run);
  if (tab && TABS.includes(tab)) params.set('tab', tab);
  if (!bind) params.set('bind', '0');
  const query = params.toString();
  return `#/a/${encodeURIComponent(id)}${query ? `?${query}` : ''}`;
}

/**
 * History-entry marker. The app records on each entry whether the entry behind it is
 * also this app's, so the in-app back button can pop instead of pushing the list.
 * An entry without a numeric `yinzIndex` is new: the first one this document sees
 * (lastIndex null) has nothing of ours behind it; later ones were created by a hash
 * navigation in this document, so the previous entry is ours. An entry that already
 * carries one is a traversal or reload and is left unchanged. Other state keys are kept.
 */
export function entryState(state, lastIndex) {
  const current = state && typeof state === 'object' ? state : null;
  if (current && Number.isInteger(current.yinzIndex)) return { state: current, lastIndex: current.yinzIndex, changed: false };
  const first = !Number.isInteger(lastIndex);
  const index = first ? 0 : lastIndex + 1;
  return { state: { ...(current || {}), yinzIndex: index, yinzBack: !first }, lastIndex: index, changed: true };
}
