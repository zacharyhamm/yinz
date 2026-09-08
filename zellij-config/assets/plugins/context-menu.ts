// Headless context_menu hook. Configured IDs retain their actions; invented items
// return their context to this plugin on selection.
import zellij, {definePlugin, type ContextMenuItem} from 'zellij:plugin';

const MAX_QUOTED = 24;
const MAX_OUTPUT = 64;
let opener: string | undefined;
let pending: string | undefined;

function shorten(text: string): string {
  const characters = [...text.replace(/\s+/gu, ' ').trim()];
  return characters.slice(0, MAX_QUOTED).join('') + (characters.length > MAX_QUOTED ? '…' : '');
}

function run(command: string[], context: Record<string, string> = {}) {
  zellij.command('RunCommand', [command, {}, '', context, MAX_OUTPUT]);
}

function openLink(url: string) {
  if (!opener) pending = url;
  else run([opener, url]);
}

zellij.on('RunCommandResult', ([exitCode, _stdout, _stderr, context]) => {
  const executable = context.probe;
  if (!executable || exitCode !== 0) return;
  if (executable === 'xdg-open' || !opener) opener = executable;
  if (pending !== undefined) {
    const url = pending;
    pending = undefined;
    openLink(url);
  }
});

zellij.on('ContextMenuRequested', event => {
  const items: ContextMenuItem[] = [];
  if (event.link?.matched) {
    items.push({label: `Open ${shorten(event.link.matched) || event.link.matched}`,
      context: {action: 'open_link', url: event.link.matched}}, {separator: true});
  }
  for (const item of event.items) {
    if (!item.separator && item.label === 'Copy') {
      if (event.selected_text == null) continue;
      const quoted = shorten(event.selected_text);
      items.push({...item, label: quoted ? `Copy "${quoted}"` : item.label});
    } else items.push(item);
  }
  const tidy: ContextMenuItem[] = [];
  for (const item of items) {
    if (!item.separator || (tidy.length > 0 && !tidy[tidy.length - 1].separator)) tidy.push(item);
  }
  if (tidy[tidy.length - 1]?.separator) tidy.pop();
  zellij.command('SetContextMenu', {request_id: event.request_id, items: tidy});
});

zellij.on('ContextMenuItemSelected', ({item_context: context}) => {
  if (context.action === 'open_link' && context.url) openLink(context.url);
});

export default definePlugin({
  load() {
    opener = undefined;
    pending = undefined;
    zellij.subscribe(['ContextMenuRequested', 'ContextMenuItemSelected', 'RunCommandResult']);
    run(['xdg-open', '--help'], {probe: 'xdg-open'});
    run(['open', '--help'], {probe: 'open'});
  },
});
