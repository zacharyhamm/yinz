// Write as hocus/todo.ts; registers todo_add, todo_list and todo_complete.
import z, {definePlugin, type Json} from 'zellij:plugin';
const namespace = 'shared:todo', D = z.data;
function args(value: Json): Record<string, Json> { if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Expected object'); return value; }
export default definePlugin({async load() {
  await z.waitForPermissions(['MessageAndLaunchOtherPlugins', 'PluginData', 'SharedPluginData']);
  D.define('todo', {title: {kind: 'Text', required: true, fullText: true}, done: {kind: 'Bool', indexed: true, default: false}}, {namespace});
  z.hocus.tools({
    add: {description: 'Add a todo to the shared list.', parameters: {type: 'object', properties: {title: {type: 'string'}}, required: ['title']}, safety: 'automatic',
      handler(value) { const title = args(value).title; if (typeof title !== 'string' || !title.trim()) return 'a todo needs a title'; D.apply([D.put('todo', {title: title.trim(), done: false})], {namespace}); return `added: ${title.trim()}`; }},
    list: {description: 'List todos; optionally filter by done.', parameters: {type: 'object', properties: {done: {type: 'boolean'}}}, safety: 'read_only',
      handler(value) { const done = args(value).done; const todos = D.fetch({entity: 'todo', namespace, filter: done == null ? null : D.eq('done', done === true), sort: [['done', 'Asc'], ['id', 'Asc']], limit: 100}).records;
        return todos.map(todo => `#${todo.id} [${todo.done ? 'x' : ' '}] ${todo.title}`).join('\n') || 'no todos'; }},
    complete: {description: 'Mark a todo done.', parameters: {type: 'object', properties: {id: {type: 'integer'}}, required: ['id']}, safety: 'automatic',
      handler(value) { const id = args(value).id; if (typeof id !== 'number' || !Number.isSafeInteger(id)) return 'complete needs the id of a todo';
        const report = D.apply([D.update('todo', D.eq('id', id), {done: true})], {namespace}); return Number(report.rows_written) === 0 ? `no todo has id ${id}` : `completed #${id}`; }},
  });
}});
