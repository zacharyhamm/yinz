// A read-only note listing with tags and todo links; never fetches note bodies.
import z, {definePlugin, type DataRecord} from 'zellij:plugin';
const namespace = 'shared:hocus', D = z.data;
let notes: DataRecord[] = [], tags: DataRecord[] = [], links: DataRecord[] = [], name = '', status = 'asking for permission';
function accept(records: DataRecord[]) {
  notes = records; const ids = notes.map(note => note.id); tags = []; links = [];
  if (ids.length) {
    tags = D.fetch({namespace, entity: 'note_tag', filter: D.in('note_id', ids), limit: 1000}).records;
    links = D.fetch({namespace, entity: 'note_link', filter: D.in('note_id', ids), limit: 1000}).records;
  }
}
export default definePlugin({
  async load() {
    try {
      await z.waitForPermissions(['PluginData', 'SharedPluginData']);
      const cwd = z.getPluginIds().initial_cwd;
      const project = D.fetch({namespace, entity: 'project'}).records.filter(p => typeof p.root === 'string' && p.root && (cwd === p.root || cwd.startsWith(p.root + '/')))
        .sort((a,b) => String(b.root).length - String(a.root).length)[0];
      if (!project) throw new Error(`no hocus project covers ${cwd}`);
      name = String(project.name);
      const watch = D.watch({namespace, entity: 'note', filter: D.and(D.eq('root', project.root), D.eq('archived', false)), sort: [['updated_at', 'Desc']], select: ['id', 'title', 'author', 'updated_at'], limit: 500}, snapshot => {accept(snapshot.records); return true;});
      accept(watch.initial.records); status = '';
    } catch (error) {status = String(error);}
    z.requestRender();
  },
  render(rows, cols) {
    if (status) return z.ui.text(status).dim().render({width: cols, height: 1});
    const list = z.ui.nestedList();
    for (const note of notes) {
      list.item(String(note.title ?? ''));
      const detail = tags.filter(tag => tag.note_id === note.id).map(tag => `#${tag.tag}`);
      const todos = links.filter(link => link.note_id === note.id).map(link => String(link.todo_id));
      if (todos.length) detail.push(`todos ${todos.join(', ')}`);
      if (note.author && note.author !== 'user') detail.push(`by ${note.author}`);
      if (detail.length) list.item(z.ui.text(detail.join(' · ')).dim(), 1);
    }
    if (!notes.length) list.item(z.ui.text('no notes').dim());
    return z.ui.text(`${name} notes`).emphasis(0).render() + '\n' + list.render({y: 1, width: cols, height: Math.max(0, rows - 1)});
  },
});
