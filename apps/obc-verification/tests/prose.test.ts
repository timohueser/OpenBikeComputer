import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { apply, type Note } from '../src/lib/prose.ts';

/** A note as the checker reports one: the offsets of the problem in the text it was given. */
function note(start: number, end: number): Note {
  return { start, end, source: '', kind: '', message: '', fixes: [] };
}

test('a writing fix rewrites only the text the note points at', () => {
  const text = 'The the rider should not recieve a error.';
  assert.equal(apply(text, note(0, 7), { label: 'The', text: 'The', insert: false }),
    'The rider should not recieve a error.');
  assert.equal(apply(text, note(25, 32), { label: 'receive', text: 'receive', insert: false }),
    'The the rider should not receive a error.');
  // A removal replaces the problem with nothing; an insertion keeps it and adds after it.
  assert.equal(apply(text, note(4, 8), { label: 'Remove', text: '', insert: false }),
    'The rider should not recieve a error.');
  assert.equal(apply(text, note(0, 3), { label: 'Add “n”', text: 'n', insert: true }),
    'Then the rider should not recieve a error.');
});
