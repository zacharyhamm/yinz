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
