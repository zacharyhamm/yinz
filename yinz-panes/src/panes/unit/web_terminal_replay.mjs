import assert from 'node:assert/strict';
import { json } from 'node:stream/consumers';
import { Terminal } from '../../../../yinz-steel-builtins/assets/plugins/web/static/vendor/xterm.mjs';
import { useNativeCharacterWidths } from '../../../../yinz-steel-builtins/assets/plugins/web/static/views/character-width.mjs';

async function replayMultipart({ parts, delta, alternate, normal, colors }) {
  const term = new Terminal({ allowProposedApi: true });
  useNativeCharacterWidths(term);
  async function apply({ response: { terminal: frame } }) {
    if (frame.snapshot) {
      term.reset();
      term.resize(frame.cols, frame.rows);
    }
    await new Promise((done) => term.write(Buffer.from(frame.data, 'base64'), done));
  }
  function compare(expected) {
    const buffer = term.buffer.active;
    const lines = Array.from({ length: buffer.length }, (_, y) => buffer.getLine(y).translateToString(true));
    assert.deepEqual([buffer.cursorX, buffer.cursorY], expected.cursor);
    assert.deepEqual(lines.join('\n').trimEnd().split('\n'), expected.text.trimEnd().split('\n'));
  }
  for (const part of parts) await apply(part);
  compare(alternate);
  await apply(delta);
  compare(normal);
  const buffer = term.buffer.normal;
  colors.forEach((row, y) => row.forEach((expected, x) => {
    if (expected) {
      const cell = buffer.getLine(buffer.baseY + y).getCell(x);
      assert.deepEqual([cell.getFgColor(), cell.getBgColor()], expected, `cell ${x},${y}`);
    }
  }));
  term.dispose();
}

for (const [name, value] of Object.entries(await json(process.stdin))) {
  if (name === 'multipart') {
    await replayMultipart(value);
    continue;
  }
  const [cols, snapshot, delta, nativeText] = value;
  const term = new Terminal({ cols, rows: 4, allowProposedApi: true });
  useNativeCharacterWidths(term);
  term.reset();
  const write = (data) => new Promise((done) => term.write(Buffer.from(data, 'base64'), done));
  const line = (y) => term.buffer.active.getLine(y);
  await write(snapshot);
  if (name === 'alternate') {
    assert.equal(line(0).translateToString(true), 'abcdefgh');
    assert.equal(line(1).translateToString(true), 'ij');
  }
  if (name === 'emoji') {
    assert.equal(line(0).getCell(0).getWidth(), 2);
    assert.equal(line(0).getCell(2).getChars(), 'A');
  }
  await write(delta);
  if (name === 'alternate') {
    assert.equal(line(0).translateToString(true), 'normal');
    assert.equal(term.buffer.active.cursorX, 6);
    assert.equal(term.buffer.active.cursorY, 0);
  } else if (name === 'emoji') {
    assert.equal(line(0).translateToString(true), '🙂B');
    assert.equal(line(0).getCell(2).getChars(), 'B');
  } else if (name === 'charsets') {
    assert.equal(line(0).translateToString(true), '──£');
  } else if (name === 'saved') {
    assert.equal(line(0).translateToString(true), '─bc');
  } else if (name === 'alternate_charset') {
    assert.equal(line(0).translateToString(true), '──');
  } else if (name === 'tabs') {
    assert.equal(line(0).getCell(3).getChars(), 'X');
    assert.equal(line(0).getCell(15).getChars(), 'Y');
  } else if (name === 'margin_charset') {
    assert.equal(line(0).translateToString(true), '1234567q');
    assert.equal(line(1).translateToString(true), '─');
  } else if (name === 'positioned') {
    assert.equal(line(0).translateToString(true), '1234567x');
    assert.equal(line(1).translateToString(true), '');
  } else {
    assert.equal(line(0).translateToString(true), name === 'narrow' ? '12345678' : '123456界');
    assert.equal(line(1).translateToString(true), 'x');
    if (name === 'wide') {
      assert.equal(line(0).getCell(6).getWidth(), 2);
      assert.equal(line(0).getCell(6).getFgColor(), 1);
      assert.equal(line(1).getCell(0).getFgColor(), 2);
    }
  }
  let rendered = '';
  for (let y = 0; y < term.rows; y++) {
    if (y && !line(y).isWrapped) rendered += '\n';
    rendered += line(y).translateToString(true);
  }
  assert.equal(rendered.trimEnd(), nativeText.trimEnd(), name);
  term.dispose();
}
