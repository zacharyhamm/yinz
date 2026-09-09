// Reader/editor for shared:todo; todo-extension.ts owns the schema.
import z, {definePlugin, type DataRecord, type Mutation} from 'zellij:plugin';
const namespace = 'shared:todo', D = z.data;
let todos: DataRecord[] = [], selected = 0, status = 'loading', input: string | undefined, editing: DataRecord | undefined;
function apply(mutation: Mutation) { try {D.apply([mutation], {namespace});} catch(error) {status = String(error);} }
z.on('Key', event => {
  const key = typeof event.bare_key === 'object' && 'Char' in event.bare_key ? event.bare_key.Char : event.bare_key;
  const todo = todos[selected];
  if (input != null) {
    if (key === 'Esc') { input = undefined; editing = undefined; }
    else if (key === 'Enter') {
      const title = input.trim();
      if (title) apply(editing ? D.update('todo', D.eq('id', editing.id), {title}, {expectRev: editing.rev}) : D.put('todo', {title, done: false}));
      input = undefined; editing = undefined;
    } else if (key === 'Backspace') input = [...input].slice(0, -1).join('');
    else if (typeof event.bare_key === 'object' && 'Char' in event.bare_key) input += event.bare_key.Char;
  } else if (key === 'q' || key === 'Esc') z.command('CloseSelf');
  else if (key === 'j' || key === 'Down') selected = Math.min(selected + 1, Math.max(0, todos.length - 1));
  else if (key === 'k' || key === 'Up') selected = Math.max(selected - 1, 0);
  else if (key === 'a') {input = ''; editing = undefined;}
  else if (key === 'e' && todo) {input = String(todo.title); editing = {...todo};}
  else if ((key === 'Enter' || key === ' ') && todo) apply(D.update('todo', D.eq('id', todo.id), {done: !todo.done}));
  else if (key === 'd' && todo) apply(D.delete('todo', D.eq('id', todo.id)));
  return true;
});
export default definePlugin({
  async load() {
    z.subscribe(['Key']);
    try {
      await z.waitForPermissions(['PluginData', 'SharedPluginData', 'ChangeApplicationState']);
      const accept = (records: DataRecord[]) => {todos = records; selected = Math.min(selected, Math.max(0, todos.length - 1)); status = `${todos.length} todos`;};
      const watch = D.watch({entity: 'todo', namespace, sort: [['done', 'Asc'], ['id', 'Asc']], limit: 200}, snapshot => {accept(snapshot.records); return true;});
      accept(watch.initial.records);
    } catch (error) {status = `cannot read the todo list: ${error}`;}
    z.requestRender();
  },
  render(_rows, cols) {
    const lines = [z.ui.text('todo').selected().render()];
    if (!todos.length) lines.push('(nothing yet - press a to add one)');
    todos.forEach((todo, index) => {let line = z.ui.text(z.text.truncate(`${todo.done ? '[x]' : '[ ]'} ${todo.title}`, Math.max(10, cols - 2))); if (todo.done) line = line.dim(); if (index === selected && input == null) line = line.selected(); lines.push(line.render());});
    lines.push('', input != null ? `${editing ? 'rename' : 'new'}: ${input}_\n<Enter> save  <Esc> cancel` : `${status}\nj/k move  <Enter> toggle  a add  e rename  d delete  q quit`);
    return lines.join('\n');
  },
});
