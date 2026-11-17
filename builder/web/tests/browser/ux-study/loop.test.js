import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { actions as browserActions } from './actions.js';
import { createBudget, walk } from './loop.js';

const model = { usdPerMillionInput: 2, usdPerMillionOutput: 10 };
const usage = { input_tokens: 100_000, output_tokens: 10_000 }; // 0.30 USD
const click = (x, y) => ({ type: 'click', target: 'map', x, y, x2: null, y2: null, text: null });

/** An environment whose screens never change, and whose app is a count of performed actions. */
function standInEnv() {
  const app = { performed: 0 };
  return {
    actions: browserActions,
    start: async () => ({
      app,
      screenshot: async () => ({ jpeg: Buffer.from('jpeg'), unstable: false }),
      perform: async () => { app.performed++; },
      describe: async () => ({ screen: 'planner', hit: 'map' }),
      end: async () => {},
    }),
  };
}

/**
 * A participant that answers with `actions` in turn, says `no` when asked whether it is done, and gives
 * `hardest` at the end, or throws it when it is an error. `seen` holds the records it got.
 */
function standInParticipant(actions, { fail = 0, hardest = { step: null, why: 'No step was unclear.' } } = {}) {
  let calls = 0;
  let asked = 0;
  const seen = [];
  return {
    seen,
    get calls() { return calls; },
    get asked() { return asked; },
    step: async ({ records }) => {
      calls++;
      seen.push(...records);
      if (calls <= fail) throw Object.assign(new Error('refusal'), { usage });
      return { value: { options: [], expect: '', action: actions.shift(), match: null, confusion: { level: 0, reason: '' } }, usage };
    },
    done: async ({ records }) => { calls++; seen.push(...records); return { value: 'no', usage }; },
    hardest: async ({ records }) => {
      asked++;
      seen.push(...records);
      if (hardest instanceof Error) throw Object.assign(hardest, { usage });
      return { value: hardest, usage };
    },
  };
}

const readRecords = dir => readFileSync(join(dir, 'steps.jsonl'), 'utf8').trim().split('\n').map(line => JSON.parse(line));

const run = ({ actions, success = () => false, budget = createBudget(model, 15), fail, hardest, stepBudget = 25 }) => {
  const env = standInEnv();
  const participant = standInParticipant(actions, { fail, hardest });
  const dir = mkdtempSync(join(tmpdir(), 'ux-study-'));
  const task = { goal: 'goal', stepBudget, success: async app => success(app) };
  return walk({ task, env, participant, budget, dir }).then(result => ({ result, participant, dir }));
};

test('three clicks at different points of the map do not end a walkthrough', async () => {
  const { result } = await run({ actions: [click(100, 100), click(400, 300), click(700, 500), { type: 'give_up' }] });
  assert.deepEqual(result, { outcome: 'gave_up', steps: 4 });
});

test('a give-up records the screen where it happened and no control', async () => {
  const { dir } = await run({ actions: [{ type: 'give_up', target: '', x: null, y: null, x2: null, y2: null, text: null }] });
  const [record] = readRecords(dir);
  assert.deepEqual([record.screen, record.hit], ['planner', null]);
});

test('the same action on the same screen three times ends as stuck', async () => {
  const { result } = await run({ actions: [click(100, 100), click(110, 105), click(95, 100), click(500, 500)] });
  assert.deepEqual(result, { outcome: 'stuck', steps: 3 });
});

test('success ends as done and stores the answer of the participant in the last record', async () => {
  const { result, dir } = await run({ actions: [click(1, 1), click(300, 300)], success: app => app.performed === 2 });
  assert.deepEqual(result, { outcome: 'done', steps: 2 });
  const records = readRecords(dir);
  assert.equal(records[1].done_answer, 'no');
  assert.deepEqual([records[1].screen, records[1].hit, records[1].unstable], ['planner', 'map', false]);
});

test('the participant names its hardest step once at the end, and only the record of that step holds the answer', async () => {
  const hardest = { step: 1, why: 'The map did not react to my click.' };
  const { participant, dir } = await run({ actions: [click(1, 1), click(300, 300), { type: 'give_up' }], hardest });
  assert.equal(participant.asked, 1);
  assert.deepEqual(readRecords(dir).map(r => r.hardest), [hardest.why, undefined, undefined]);
});

test('a failed end question keeps the outcome of the walkthrough, and its calls count toward the cost', async () => {
  const budget = createBudget(model, 15);
  const { result, participant, dir } = await run({ actions: [{ type: 'give_up' }], hardest: new Error('refusal'), budget });
  assert.deepEqual(result, { outcome: 'gave_up', steps: 1 });
  assert.equal(participant.asked, 3);
  assert.equal(readRecords(dir)[0].hardest, undefined);
  assert.equal(budget.spent.toFixed(2), '1.20');
});

test('the participant gets back only its own fields, never the fields that the loop adds from the page', async () => {
  const { participant } = await run({ actions: [click(1, 1), click(300, 300)], success: app => app.performed === 2,
    hardest: { step: 1, why: 'why' } });
  assert.ok(participant.seen.length > 0);
  for (const record of participant.seen) assert.deepEqual(Object.keys(record).sort(), ['action', 'confusion', 'expect', 'match', 'options']);
});

test('an action without the fields its type needs counts as a failed call and gets another try', async () => {
  const key = { type: 'key', target: 'Enter', x: null, y: null, x2: null, y2: null, text: null };
  const { result, participant } = await run({ actions: [key, { type: 'give_up' }] });
  assert.deepEqual(result, { outcome: 'gave_up', steps: 1 });
  assert.equal(participant.calls, 2);
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
