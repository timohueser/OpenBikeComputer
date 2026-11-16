import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { createBudget } from './loop.js';
import { candidateSteps, keptGroups, writeReport } from './report.js';

const SEARCH = 'textbox "Find a place, or ask along your route…"';
const CHIP = 'button "In this map view"';
const RESULT = 'button "Meiringen train station · Meiringen"';

/** Step records as the run loop writes them: `[screen, hit, match, confusion]` for each step. */
const records = steps => steps.map(([screen, hit, match = 'yes', confusion = 0], i) => ({
  step: i + 1, options: [], expect: '', action: { type: 'click', target: '', x: 1, y: 1, x2: null, y2: null, text: null },
  match: i ? match : null, confusion: { level: confusion, reason: 'reason' }, screen, hit, unstable: false,
}));

/** Three walkthroughs that all completed the task; the second and third were surprised by the search box. */
const ordinary = [
  { task: 'plan-route', n: 1, outcome: 'done', records: records([['planner', SEARCH], ['planner', RESULT], ['planner', CHIP, 'no']]) },
  { task: 'plan-route', n: 2, outcome: 'done', records: records([['planner', SEARCH], ['planner', RESULT, 'no'], ['planner', 'map']]) },
  { task: 'plan-route', n: 3, outcome: 'done', records: records([['planner', CHIP], ['planner', SEARCH], ['planner', RESULT, 'no']]) },
];

test('the candidate steps are the steps whose result did not match, the confusing screens, the end of a failed walkthrough, and a done answer of no', () => {
  const doneNo = records([['planner', SEARCH], ['planner', 'map', 'yes', 2]]);
  doneNo[1].done_answer = 'no';
  const steps = candidateSteps([
    ...ordinary,
    { task: 'plan-route', n: 4, outcome: 'done', records: doneNo },
    { task: 'plan-route', n: 5, outcome: 'gave_up', records: records([['planner', SEARCH], ['planner', CHIP], ['planner', 'map'], ['planner', 'map']]) },
    { task: 'plan-route', n: 6, outcome: 'error', records: records([['planner', SEARCH], ['planner', RESULT, 'no']]) },
  ]);
  assert.deepEqual(steps.map(s => [s.id, s.hit, s.atEnd, s.reasons.length]), [
    ['plan-route-1/2', RESULT, false, 1],
    ['plan-route-2/1', SEARCH, false, 1],
    ['plan-route-3/2', SEARCH, false, 1],
    ['plan-route-4/2', 'map', false, 2],
    ['plan-route-5/2', CHIP, true, 1],
    ['plan-route-5/3', 'map', true, 1],
    ['plan-route-5/4', 'map', true, 1],
  ]);
});

test('a group stays when two walkthroughs show it or a walkthrough failed there, failed groups first', () => {
  const failed = { task: 'plan-route', n: 4, outcome: 'stuck', records: records([['planner', 'map'], ['planner', 'map'], ['planner', 'map']]) };
  const groups = keptGroups(candidateSteps([...ordinary, failed]));
  assert.deepEqual(groups.map(g => [g.hit, g.walkthroughs, g.failed, g.steps.map(s => s.id)]), [
    ['map', 1, true, ['plan-route-4/1', 'plan-route-4/2', 'plan-route-4/3']],
    [SEARCH, 2, false, ['plan-route-2/1', 'plan-route-3/2']],
  ]);
  const many = Array.from({ length: 20 }, (_, n) =>
    ({ task: 'plan-route', n, outcome: 'gave_up', records: records([[`screen ${n}`, 'map']]) }));
  assert.equal(keptGroups(candidateSteps(many)).length, 15);
});

test('the report links each step of a finding to its screenshot, and its first line says when gh failed', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'ux-report-'));
  for (const { task, n, records: steps } of ordinary) {
    mkdirSync(join(dir, `${task}-${n}`));
    writeFileSync(join(dir, `${task}-${n}`, 'steps.jsonl'), steps.map(r => `${JSON.stringify(r)}\n`).join(''));
  }
  const budget = createBudget({ usdPerMillionInput: 2, usdPerMillionOutput: 10 }, 15);
  const finding = { title: 'The search box finds nothing', where: 'planner · search', what_happened: 'It shows no results.', direction: 'Search as the rider types.', steps: ['plan-route-3/2', 'plan-route-2/1', 'unknown/1'] };
  const path = await writeReport({
    dir, budget, model: { id: 'model' }, maxCostUsd: 15, limited: false, commit: 'abc1234',
    results: ordinary.map(({ task, n, outcome }) => ({ task, n, outcome })),
    ask: async () => ({ value: { findings: [finding], seen_again: [] }, usage: { input_tokens: 100_000, output_tokens: 10_000 } }),
    issues: async () => ({ error: 'gh: not logged in' }),
  });
  const lines = readFileSync(path, 'utf8').split('\n');
  assert.equal(lines[0], '`gh` failed, so the report does not list open findings: gh: not logged in');
  assert.equal(lines[2], '# UX study · planner · abc1234 · model · 0.30 USD');
  assert.ok(lines.includes('- Evidence: plan-route 2, step [1](plan-route-2/01.png); plan-route 3, step [2](plan-route-3/02.png)'));
  assert.ok(!lines.some(line => line.startsWith('## Open findings')));
});
