import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { listEnter } from '../src/lib/markdown-keys.ts';

/** The text with the edit applied, and where the caret lands. */
function press(text: string): string {
  const at = text.indexOf('|');
  const value = text.replace('|', '');
  const edit = listEnter(value, at);
  if (!edit) return `${value.slice(0, at)}\n|${value.slice(at)}`;
  const next = value.slice(0, edit.from) + edit.text + value.slice(edit.to);
  return `${next.slice(0, edit.from + edit.text.length)}|${next.slice(edit.from + edit.text.length)}`;
}

test('Enter carries a Markdown list on, and ends it on an empty item', () => {
  assert.equal(press('- Steps|'), '- Steps\n- |');
  assert.equal(press('  * Nested|'), '  * Nested\n  * |');
  assert.equal(press('3. Third|'), '3. Third\n4. |');
  assert.equal(press('9) Ninth|'), '9) Ninth\n10) |');
  assert.equal(press('- [x] Done|'), '- [x] Done\n- [ ] |');
  // An item with no text of its own ends the list rather than making another empty item.
  assert.equal(press('- One\n- |'), '- One\n|');
  assert.equal(press('- [ ] |'), '|');
  // Splitting an item keeps the rest of its text on the new one.
  assert.equal(press('- One|two'), '- One\n- |two');
  // Outside a list, and inside the marker itself, Enter stays Enter.
  assert.equal(press('A sentence.|'), 'A sentence.\n|');
  assert.equal(press('-| item'), '-\n| item');
});
