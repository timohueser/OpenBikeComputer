import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { createBudget } from './loop.js';
import { candidateSteps, keptGroups, MAX_IMAGE_BYTES, writeReport } from './report.js';

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

test('the candidate steps are the steps with a strong or a mild signal and the end of a failed walkthrough', () => {
  const doneNo = records([['planner', SEARCH], ['planner', 'map', 'yes', 2]]);
  doneNo[1].done_answer = 'no';
  const mild = records([['planner', SEARCH], ['planner', CHIP, 'partly', 1]]);
  mild[0].hardest = 'The search box looked like a title.';
  const steps = candidateSteps([
    ...ordinary,
    { task: 'plan-route', n: 4, outcome: 'done', records: doneNo },
    { task: 'plan-route', n: 5, outcome: 'gave_up', records: records([['planner', SEARCH], ['planner', CHIP], ['planner', 'map'], ['planner', 'map']]) },
    { task: 'plan-route', n: 6, outcome: 'error', records: records([['planner', SEARCH], ['planner', RESULT, 'no']]) },
    { task: 'plan-route', n: 7, outcome: 'done', records: mild },
  ]);
  assert.deepEqual(steps.map(s => [s.id, s.hit, s.atEnd, s.strong, s.reasons]), [
    ['plan-route-1/2', RESULT, false, true, ['the result did not match the expectation']],
    ['plan-route-2/1', SEARCH, false, true, ['the result did not match the expectation']],
    ['plan-route-3/2', SEARCH, false, true, ['the result did not match the expectation']],
    ['plan-route-4/2', 'map', false, true, ['confusion 2: reason', 'the participant did not see that the task was done']],
    ['plan-route-5/2', CHIP, true, false, ['one of the last steps before gave_up']],
    ['plan-route-5/3', 'map', true, false, ['one of the last steps before gave_up']],
    ['plan-route-5/4', 'map', true, false, ['one of the last steps before gave_up']],
    ['plan-route-7/1', SEARCH, false, false, ['named as the hardest step', 'the result matched the expectation only partly']],
    ['plan-route-7/2', CHIP, false, false, ['confusion 1: reason']],
  ]);
});

test('the give-ups of failed walkthroughs group by the screen where they happened', () => {
  const gaveUp = (n, screen) => {
    const steps = records([['planner', SEARCH], [screen, null]]);
    steps[1].action = { ...steps[1].action, type: 'give_up' };
    return { task: 'plan-route', n, outcome: 'gave_up', records: steps };
  };
  const groups = keptGroups(candidateSteps([gaveUp(1, 'planner'), gaveUp(2, 'planner · dialog "Map details"')]));
  assert.deepEqual(groups.map(g => [g.screen, g.hit, g.walkthroughs]).filter(([, hit]) => hit === null),
    [['planner', null, 1], ['planner · dialog "Map details"', null, 1]]);
});

test('a group stays when two walkthroughs show it or a walkthrough failed there; failed groups first, then groups with a strong signal', () => {
  const doubt = w => ({ ...w, records: w.records.map(r => (r.hit === CHIP ? { ...r, confusion: { level: 1, reason: 'reason' } } : r)) });
  const chip = { task: 'plan-route', n: 4, outcome: 'done', records: records([['planner', CHIP, 'yes', 1]]) };
  const failed = { task: 'plan-route', n: 5, outcome: 'stuck', records: records([['planner', 'map'], ['planner', 'map'], ['planner', 'map']]) };
  const groups = keptGroups(candidateSteps([...ordinary.map(doubt), chip, failed]));
  assert.deepEqual(groups.map(g => [g.hit, g.walkthroughs, g.failed, g.strong, g.steps.map(s => s.id)]), [
    ['map', 1, true, false, ['plan-route-5/1', 'plan-route-5/2', 'plan-route-5/3']],
    [SEARCH, 2, false, true, ['plan-route-2/1', 'plan-route-3/2']],
    [CHIP, 3, false, false, ['plan-route-1/3', 'plan-route-3/1', 'plan-route-4/1']],
  ]);
  const many = Array.from({ length: 20 }, (_, n) =>
    ({ task: 'plan-route', n, outcome: 'gave_up', records: records([[`screen ${n}`, 'map']]) }));
  assert.equal(keptGroups(candidateSteps(many)).length, 15);
});

/** Writes the walkthroughs to a new study folder and the report of `options`; resolves with its lines and the calls of `ask`. */
async function report({ walkthroughs = ordinary, ask, limit = 15, spent = 0, limited = false, issues, screenshots = {} } = {}) {
  const dir = mkdtempSync(join(tmpdir(), 'ux-report-'));
  for (const { task, n, records: steps } of walkthroughs) {
    mkdirSync(join(dir, `${task}-${n}`));
    writeFileSync(join(dir, `${task}-${n}`, 'steps.jsonl'), steps.map(r => `${JSON.stringify(r)}\n`).join(''));
  }
  for (const [file, bytes] of Object.entries(screenshots)) writeFileSync(join(dir, file), Buffer.alloc(bytes));
  const budget = createBudget({ usdPerMillionInput: 2, usdPerMillionOutput: 10 }, limit);
  budget.add({ input_tokens: spent * 500_000, output_tokens: 0 });
  const calls = [];
  const path = await writeReport({
    dir, budget, model: { id: 'model' }, maxCostUsd: limit, limited, commit: 'abc1234',
    results: walkthroughs.map(({ task, n, outcome }) => ({ task, n, outcome })),
    ask: async (...call) => { calls.push(call); return ask(...call); },
    issues: issues ?? (async () => ({ issues: [{ number: 7, title: 'The bike button has no label', body: '' }] })),
  });
  return { lines: readFileSync(path, 'utf8').split('\n'), calls };
}

const answers = finding => async () => ({ value: { findings: [finding], seen_again: [] }, usage: { input_tokens: 100_000, output_tokens: 10_000 } });
const openSections = lines => lines.filter(line => line.startsWith('## Open findings'));

test('the report counts the walkthroughs of a finding from its steps, links each step to its screenshot, and says when gh failed', async () => {
  const finding = { title: 'The search box finds nothing', where: 'planner · search', what_happened: 'It shows no results.', direction: 'Search as the rider types.', steps: ['plan-route-3/2', 'plan-route-2/1', 'unknown/1'] };
  const { lines } = await report({ ask: answers(finding), issues: async () => ({ error: 'gh: not logged in' }) });
  assert.equal(lines[0], '`gh` failed, so the report does not list open findings: gh: not logged in');
  assert.equal(lines[2], '# UX study · planner · abc1234 · model · 0.30 USD');
  assert.ok(lines.includes('- Evidence (2 of 3 plan-route walkthroughs): plan-route 2, step [1](plan-route-2/01.jpg); plan-route 3, step [2](plan-route-3/02.jpg)'));
  assert.deepEqual(openSections(lines), []);
});

test('the report counts one walkthrough and one open finding in the singular', async () => {
  const { lines } = await report({ ask: answers(), walkthroughs: ordinary.slice(0, 1) });
  assert.equal(lines[2], '1 of 1 walkthrough completed its task. 1 accepted finding is open.');
});

test('a report compares the open findings only when the study completed and the report call answered', async () => {
  const answered = await report({ ask: answers({ title: 't', where: 'w', what_happened: 'h', direction: 'd', steps: ['plan-route-2/1'] }) });
  assert.deepEqual(openSections(answered.lines), ['## Open findings seen again', '## Open findings not seen in this study']);

  const failed = await report({ ask: async () => { throw new Error('overloaded'); } });
  assert.equal(failed.lines[0], 'The report call failed, so the report has no new findings: overloaded');
  assert.deepEqual(openSections(failed.lines), []);

  const skipped = await report({ ask: answers(), limit: 1, spent: 1, limited: true });
  assert.equal(skipped.lines[0], 'The spending limit of 1 USD stopped the study. The spending limit is reached, so the report call did not run.');
  assert.equal(skipped.calls.length, 0);
  assert.deepEqual(openSections(skipped.lines), []);

  const allErrors = await report({ ask: answers(), walkthroughs: ordinary.map(w => ({ ...w, outcome: 'error', records: [] })) });
  assert.equal(allErrors.calls.length, 0);
  assert.deepEqual(openSections(allErrors.lines), []);
});

test('the report call gets the design principles of PRODUCT.md', async () => {
  const { calls } = await report({ ask: answers({ title: 't', where: 'w', what_happened: 'h', direction: 'd', steps: ['plan-route-2/1'] }) });
  const principles = readFileSync(new URL('../../../../../PRODUCT.md', import.meta.url), 'utf8');
  const [[content]] = calls;
  assert.ok(content.some(block => block.type === 'text' && block.text.endsWith(principles)));
});

test('the report call does not start when its screenshots pass the limit of the request', async () => {
  const half = MAX_IMAGE_BYTES / 2;
  const { lines, calls } = await report({ ask: answers(), screenshots: { 'plan-route-2/01.jpg': half, 'plan-route-2/02.jpg': half } });
  assert.equal(calls.length, 0);
  assert.match(lines[0], /^The report call failed, so the report has no new findings: the screenshots are \d+ bytes, more than 16000000$/);
});
