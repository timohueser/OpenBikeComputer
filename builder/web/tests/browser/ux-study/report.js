// The report of a study: selects the steps where walkthroughs failed or the participant was surprised,
// groups them by place, has the model write the findings in one call, and writes `report.md`.
import { execFile, execFileSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { createModelCall, image } from './model.js';

const PROMPT = readFileSync(new URL('./report.md', import.meta.url), 'utf8');
const REJECTED = new URL('./rejected.md', import.meta.url);
const MIN_WALKTHROUGHS = 2;
const MAX_GROUPS = 15;
const MAX_FINDINGS = 10;
const END_STEPS = 3;

const findingsSchema = {
  type: 'object',
  properties: {
    findings: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          title: { type: 'string' },
          where: { type: 'string' },
          what_happened: { type: 'string' },
          direction: { type: 'string' },
          steps: { type: 'array', items: { type: 'string' } },
        },
        required: ['title', 'where', 'what_happened', 'direction', 'steps'],
        additionalProperties: false,
      },
    },
    seen_again: { type: 'array', items: { type: 'integer' } },
  },
  required: ['findings', 'seen_again'],
  additionalProperties: false,
};

const failed = outcome => outcome === 'stuck' || outcome === 'gave_up';
const screenFile = n => `${String(n).padStart(2, '0')}.png`;

/**
 * The candidate steps of `walkthroughs` (`{ task, n, outcome, records }`). A step is a candidate when the
 * next step says that its result did not match the expectation, when its screen confused the participant
 * most, when it is one of the last three steps of a failed walkthrough, or when the participant did not see
 * that its task was done. An `error` walkthrough gives no candidates.
 */
export function candidateSteps(walkthroughs) {
  const candidates = new Map();
  for (const { task, n, outcome, records } of walkthroughs) {
    if (outcome === 'error') continue;
    const add = (record, reason) => {
      const id = `${task}-${n}/${record.step}`;
      const candidate = candidates.get(id)
        ?? { id, task, n, outcome, step: record.step, screen: record.screen, hit: record.hit, reasons: [], atEnd: false };
      candidate.reasons.push(reason);
      candidates.set(id, candidate);
      return candidate;
    };
    records.forEach((record, i) => {
      // `match` judges the action of the step before.
      if (record.match === 'no' && i > 0) add(records[i - 1], 'the result did not match the expectation');
      if (record.confusion.level === 2) add(record, `confusion 2: ${record.confusion.reason}`);
      if (record.done_answer === 'no') add(record, 'the participant did not see that the task was done');
      if (failed(outcome) && i >= records.length - END_STEPS) add(record, `one of the last steps before ${outcome}`).atEnd = true;
    });
  }
  return [...candidates.values()];
}

/**
 * The candidates grouped by task, `screen` and `hit`. A group stays when at least two walkthroughs show it,
 * or when a walkthrough failed there. At most 15 stay: those with a failed walkthrough first, then those that
 * more walkthroughs show.
 */
export function keptGroups(candidates) {
  const groups = new Map();
  for (const candidate of candidates) {
    const key = JSON.stringify([candidate.task, candidate.screen, candidate.hit]);
    if (!groups.has(key)) groups.set(key, { task: candidate.task, screen: candidate.screen, hit: candidate.hit, steps: [] });
    groups.get(key).steps.push(candidate);
  }
  return [...groups.values()]
    .map(group => ({ ...group, walkthroughs: new Set(group.steps.map(s => s.n)).size, failed: group.steps.some(s => s.atEnd) }))
    .filter(group => group.walkthroughs >= MIN_WALKTHROUGHS || group.failed)
    .sort((a, b) => b.failed - a.failed || b.walkthroughs - a.walkthroughs)
    .slice(0, MAX_GROUPS);
}

/** The open accepted findings, or the reason that `gh` could not list them. */
export async function openFindings() {
  try {
    // REST, because cloud sessions block the GraphQL API that `gh issue list` uses. `gh` fills in the
    // repository from the remote. The list also holds pull requests.
    const { stdout } = await promisify(execFile)('gh', ['api', 'repos/{owner}/{repo}/issues?labels=ux-finding&state=open&per_page=100']);
    return { issues: JSON.parse(stdout).filter(item => !item.pull_request).map(({ number, title, body }) => ({ number, title, body })) };
  } catch (error) {
    return { error: (error.stderr || error.message).trim().split('\n')[0].slice(0, 160) };
  }
}

/**
 * Writes `<dir>/report.md` for the walkthroughs in `results` and resolves with its path. The cost of the
 * report call goes into `budget`. `limited` says that the spending limit stopped the study.
 */
export async function writeReport({ dir, results, budget, model, maxCostUsd, limited, ask, issues = openFindings, commit = gitCommit() }) {
  const walkthroughs = results.map(result => ({ ...result, records: readRecords(join(dir, `${result.task}-${result.n}`)) }));
  const groups = keptGroups(candidateSteps(walkthroughs));
  const open = await issues();
  const notices = [];
  if (limited) notices.push(`The spending limit of ${maxCostUsd} USD stopped the study.`);
  if (open.error) notices.push(`\`gh\` failed, so the report does not list open findings: ${open.error}`);

  let answer = { findings: [], seen_again: [] };
  if (groups.length) {
    ask ??= createModelCall({ model, system: PROMPT, effort: 'medium' });
    try {
      const { value, usage } = await ask(reportContent(dir, groups, walkthroughs, open), findingsSchema);
      budget.add(usage);
      answer = value;
    } catch (error) {
      if (error.usage) budget.add(error.usage);
      notices.push(`The report call failed, so the report has no new findings: ${error.message}`);
    }
  }

  const path = join(dir, 'report.md');
  writeFileSync(path, render({ notices, commit, model, costUsd: budget.spent, walkthroughs, groups, answer, open }));
  return path;
}

function readRecords(walkDir) {
  const file = join(walkDir, 'steps.jsonl');
  return existsSync(file) ? readFileSync(file, 'utf8').split('\n').filter(Boolean).map(line => JSON.parse(line)) : [];
}

function gitCommit() {
  try { return execFileSync('git', ['rev-parse', '--short', 'HEAD'], { encoding: 'utf8' }).trim(); } catch { return 'unknown commit'; }
}

/** The groups with their step records and the two screenshots of one step, the open findings and the rejected ones. */
function reportContent(dir, groups, walkthroughs, open) {
  const records = Object.fromEntries(walkthroughs.map(w => [`${w.task}-${w.n}`, w.records]));
  const content = [];
  groups.forEach((group, i) => {
    const lines = group.steps.map(({ id, task, n, step, outcome, reasons }) => {
      const [record, next] = [records[`${task}-${n}`][step - 1], records[`${task}-${n}`][step]];
      return [`${id} (walkthrough ended ${outcome}; ${reasons.join('; ')})`, `record: ${JSON.stringify(record)}`,
        `next step: ${next ? JSON.stringify({ match: next.match, confusion: next.confusion }) : 'none'}`].join('\n');
    });
    content.push({ type: 'text', text: `Group ${i + 1}: task ${group.task}, screen ${group.screen}, control ${group.hit}, `
      + `${group.walkthroughs} walkthroughs${group.failed ? ', a walkthrough failed here' : ''}.\n${lines.join('\n\n')}` });
    const { task, n, step, id } = group.steps[0];
    for (const [label, file] of [['before', screenFile(step)], ['after', screenFile(step + 1)]]) {
      const png = join(dir, `${task}-${n}`, file);
      if (existsSync(png)) content.push({ type: 'text', text: `The screen ${label} the action of ${id}:` }, image(readFileSync(png)));
    }
  });
  content.push({ type: 'text', text: open.issues ? `Open accepted findings:\n${JSON.stringify(open.issues)}` : 'The open accepted findings are not available.' });
  content.push({ type: 'text', text: `Rejected findings:\n${readFileSync(REJECTED, 'utf8')}` });
  return content;
}

function render({ notices, commit, model, costUsd, walkthroughs, groups, answer, open }) {
  const byId = new Map(groups.flatMap(group => group.steps).map(step => [step.id, step]));
  const findings = answer.findings
    .map(finding => ({ ...finding, steps: finding.steps.filter(id => byId.has(id)).map(id => byId.get(id)) }))
    .filter(finding => finding.steps.length)
    .map(finding => ({ ...finding, failed: finding.steps.some(s => s.atEnd), walkthroughs: new Set(finding.steps.map(s => `${s.task}-${s.n}`)).size }))
    .sort((a, b) => b.failed - a.failed || b.walkthroughs - a.walkthroughs)
    .slice(0, MAX_FINDINGS);

  const tasks = [...new Set(walkthroughs.map(w => w.task))];
  const done = list => list.filter(w => w.outcome === 'done').length;
  const errors = walkthroughs.filter(w => w.outcome === 'error').length;
  const lines = [];
  if (notices.length) lines.push(notices.join(' '), '');
  lines.push(`# UX study · planner · ${commit} · ${model.id} · ${costUsd.toFixed(2)} USD`, '');
  lines.push([`${done(walkthroughs)} of ${walkthroughs.length} walkthroughs completed their task.`,
    errors && `${errors} ended with an error.`,
    open.issues && `${open.issues.length} accepted findings are open.`].filter(Boolean).join(' '), '');
  lines.push('| Task | Completed |', '| --- | --- |');
  for (const task of tasks) {
    const list = walkthroughs.filter(w => w.task === task);
    lines.push(`| ${task} | ${done(list)} of ${list.length} |`);
  }
  lines.push('', '## New findings', '');
  if (!findings.length) lines.push('None.', '');
  findings.forEach((finding, i) => lines.push(`### ${i + 1}. ${finding.title}`,
    `- Where: ${finding.where}`,
    `- What happened: ${finding.what_happened}`,
    `- Evidence: ${evidence(finding.steps)}`,
    `- Direction: ${finding.direction}`, ''));
  if (open.issues) {
    const seen = new Set(answer.seen_again);
    const list = (issues, suffix = '') => (issues.length ? issues.map(issue => `- #${issue.number}: ${issue.title}${suffix}`) : ['None.']);
    lines.push('## Open findings seen again', '', ...list(open.issues.filter(issue => seen.has(issue.number))), '');
    lines.push('## Open findings not seen in this study', '', ...list(open.issues.filter(issue => !seen.has(issue.number)), '. Close it if the fix is in.'), '');
  }
  return lines.join('\n');
}

/** "plan-route 1, steps [6](plan-route-1/06.png), [7](plan-route-1/07.png); plan-route 2, step [3](…)": each link opens the screen that the step acted on. */
function evidence(steps) {
  const walks = new Map();
  for (const s of [...steps].sort((a, b) => a.task.localeCompare(b.task) || a.n - b.n || a.step - b.step)) {
    const key = `${s.task}-${s.n}`;
    if (!walks.has(key)) walks.set(key, { label: `${s.task} ${s.n}`, links: [] });
    walks.get(key).links.push(`[${s.step}](${key}/${screenFile(s.step)})`);
  }
  return [...walks.values()].map(({ label, links }) => `${label}, ${links.length > 1 ? 'steps' : 'step'} ${links.join(', ')}`).join('; ');
}
