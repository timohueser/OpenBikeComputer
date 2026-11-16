import assert from 'node:assert/strict';
import { test } from 'node:test';
import { actions } from './actions.js';

const click = (x, y) => ({ type: 'click', target: 'map', x, y, x2: null, y2: null, text: null });

test('two actions are the same when their type and text match and their points are at most 20 pixels apart', () => {
  assert.ok(actions.same(click(100, 100), click(112, 116)));
  assert.ok(!actions.same(click(100, 100), click(112, 117)));
  assert.ok(!actions.same(click(100, 100), { ...click(100, 100), type: 'hover' }));
  assert.ok(!actions.same(click(100, 100), click(null, null)));
  const key = text => ({ type: 'key', target: text, x: null, y: null, x2: null, y2: null, text });
  assert.ok(actions.same(key('Enter'), key('Enter')));
  assert.ok(!actions.same(key('Enter'), key('Tab')));
});
