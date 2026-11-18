// Hash routes (DOM-free):
//   #/                                         agents list
//   #/a/<encodeURIComponent(id)>?run=<run>&tab=timeline|screen|details[&bind=0]
// `bind=0` marks stored-run browsing (previous/original run links), which reads
// the run without the live agent binding. Routes never carry credentials.

import { targetOf, validTarget } from './submission.js';

export const TABS = ['timeline', 'screen', 'details'];

const PEER_DETAIL_PREFIX = 'peer-detail:';

export function isPeerDetail(id) {
  return typeof id === 'string' && id.startsWith(PEER_DETAIL_PREFIX);
}

export function isScopedPeer(id) {
  return typeof id === 'string' && /^peer-detail:v1:[0-9a-f]{64}$/.test(id);
}

function peerRow(row, servingSession) {
  if (typeof row.current_session === 'boolean') return !row.current_session;
  const owner = typeof row.tab_group_session === 'string' && row.tab_group_session.trim() !== ''
    ? row.tab_group_session : row.session;
  return typeof servingSession === 'string' && typeof owner === 'string' && owner.trim() !== '' &&
    owner !== servingSession;
}

export function controlTarget(row, servingSession) {
  if (!validTarget(row?.ref)) return null;
  if (peerRow(row, servingSession)) {
    if (!isScopedPeer(row.detail_id) || row.ref.id !== row.detail_id) return null;
  } else if (row.ref.id !== row.id || isPeerDetail(row.ref.id)) return null;
  return targetOf(row.ref);
}

export function cardDetail(row, servingSession) {
  const peer = peerRow(row, servingSession);
  const scoped = peer && isScopedPeer(row.detail_id);
  if (peer && !scoped && !row.run) return null;
  return {
    id: scoped ? row.detail_id : row.id,
    run: row.run,
    tab: row.run && row.state !== 'waiting' ? 'timeline' : 'screen',
    bind: scoped || !peer,
  };
}

export function detailListRow(rows, id, servingSession) {
  if (isPeerDetail(id) && !isScopedPeer(id)) return null;
  const scoped = isScopedPeer(id);
  const matches = rows.filter((row) => row &&
    (scoped ? peerRow(row, servingSession) && row.detail_id === id
      : !peerRow(row, servingSession) && row.id === id));
  return matches.length === 1 ? matches[0] : null;
}

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
