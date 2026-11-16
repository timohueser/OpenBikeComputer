import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { createBudget, walk } from './loop.js';

const model = { usdPerMillionInput: 2, usdPerMillionOutput: 10 };
const usage = { input_tokens: 100_000, output_tokens: 10_000 }; // 0.30 USD
const click = (x, y) => ({ type: 'click', target: 'map', x, y, x2: null, y2: null, text: null });

/** An environment whose screens never change, and whose app is a count of performed actions. */
function standInEnv() {
  const app = { performed: 0 };
  return {
    actions: { same: (a, b) => a.type === b.type && Math.hypot(a.x - b.x, a.y - b.y) <= 20 },
    start: async () => ({
      app,
      screenshot: async () => ({ png: Buffer.from('png'), unstable: false }),
      perform: async () => { app.performed++; },
      describe: async () => ({ screen: 'planner', hit: 'map' }),
      end: async () => {},
    }),
  };
}

/** A participant that answers with `actions` in turn and says `done` when asked. */
function standInParticipant(actions, { fail = 0 } = {}) {
  let calls = 0;
  return {
    get calls() { return calls; },
    step: async () => {
      calls++;
      if (calls <= fail) throw Object.assign(new Error('refusal'), { usage });
      return { value: { options: [], expect: '', action: actions.shift(), match: null, confusion: { level: 0, reason: '' } }, usage };
    },
    done: async () => { calls++; return { value: 'no', usage }; },
  };
}

const run = ({ actions, success = () => false, budget = createBudget(model, 15), fail, stepBudget = 25 }) => {
  const env = standInEnv();
  const participant = standInParticipant(actions, { fail });
  const dir = mkdtempSync(join(tmpdir(), 'ux-study-'));
  const task = { goal: 'goal', stepBudget, success: async app => success(app) };
  return walk({ task, env, participant, budget, dir }).then(result => ({ result, participant, dir }));
};

test('three clicks at different points of the map do not end a walkthrough', async () => {
  const { result } = await run({ actions: [click(100, 100), click(400, 300), click(700, 500), { type: 'give_up' }] });
  assert.deepEqual(result, { outcome: 'gave_up', steps: 4 });
});

test('the same action on the same screen three times ends as stuck', async () => {
  const { result } = await run({ actions: [click(100, 100), click(110, 105), click(95, 100), click(500, 500)] });
  assert.deepEqual(result, { outcome: 'stuck', steps: 3 });
});

test('success ends as done and stores the answer of the participant in the last record', async () => {
  const { result, dir } = await run({ actions: [click(1, 1), click(300, 300)], success: app => app.performed === 2 });
  assert.deepEqual(result, { outcome: 'done', steps: 2 });
  const records = readFileSync(join(dir, 'steps.jsonl'), 'utf8').trim().split('\n').map(line => JSON.parse(line));
  assert.equal(records[1].done_answer, 'no');
  assert.deepEqual([records[1].screen, records[1].hit, records[1].unstable], ['planner', 'map', false]);
});

test('the end of the step budget ends as gave_up', async () => {
  const { result } = await run({ actions: [click(1, 1), click(300, 300), click(600, 600)], stepBudget: 2 });
  assert.deepEqual(result, { outcome: 'gave_up', steps: 2 });
});

test('a call that fails three times ends as error, and the failed calls count toward the cost', async () => {
  const budget = createBudget(model, 15);
  const { result, participant } = await run({ actions: [click(1, 1)], fail: 3, budget });
  assert.deepEqual(result, { outcome: 'error', steps: 0, reason: 'refusal' });
  assert.equal(participant.calls, 3);
  assert.equal(budget.spent.toFixed(2), '0.90');
});

test('the spending limit stops the participant calls and ends the walkthrough as error', async () => {
  const budget = createBudget(model, 0.6);
  const { result, participant } = await run({ actions: [click(1, 1), click(300, 300), click(600, 600)], budget });
  assert.deepEqual(result, { outcome: 'error', steps: 2, reason: 'spending limit' });
  assert.equal(participant.calls, 2);
  assert.ok(budget.exhausted);
});
