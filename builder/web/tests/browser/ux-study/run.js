// The command of a study: `node ux-study/run.js [--task ID]` runs each task (or one) three times at once
// and writes the walkthroughs to `.artifacts/ux-study/<UTC time>/`.
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { openBrowser } from './environment.js';
import { createBudget, walk } from './loop.js';
import { createParticipant } from './participant.js';
import { maxCostUsd, model, tasks, walkthroughsPerTask } from './study.js';

const ARTIFACTS = fileURLToPath(new URL('../../../../../.artifacts/ux-study', import.meta.url));

/** Runs the study and resolves with its folder, the result of each walkthrough and the cost. */
export async function run({ taskId, participant, walkthroughs = walkthroughsPerTask } = {}) {
  const selected = taskId ? tasks.filter(task => task.id === taskId) : tasks;
  if (!selected.length) throw new Error(`no task '${taskId}'; the tasks are: ${tasks.map(task => task.id).join(', ')}`);
  const dir = join(ARTIFACTS, new Date().toISOString().replace(/[:.]/g, '-'));
  const budget = createBudget(model, maxCostUsd);
  const env = await openBrowser();
  const results = [];
  try {
    participant ??= createParticipant({ model, actions: env.actions });
    for (const task of selected) {
      results.push(...await Promise.all(Array.from({ length: walkthroughs }, (_, i) =>
        walk({ task, env, participant, budget, dir: join(dir, `${task.id}-${i + 1}`) })
          .catch(error => ({ outcome: 'error', reason: error.message }))
          .then(result => ({ task: task.id, n: i + 1, ...result })))));
    }
  } finally {
    await env.close();
  }
  return { dir, results, costUsd: budget.spent, limited: budget.exhausted };
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const { values } = parseArgs({ options: { task: { type: 'string' } } });
  const started = Date.now();
  const { dir, results, costUsd, limited } = await run({ taskId: values.task });
  if (limited) console.log(`The spending limit of ${maxCostUsd} USD stopped the study.`);
  for (const r of results) console.log(`${r.task}-${r.n}: ${r.outcome} after ${r.steps ?? 0} steps${r.reason ? ` (${r.reason})` : ''}`);
  console.log(`${costUsd.toFixed(2)} USD, ${Math.round((Date.now() - started) / 60_000)} min, ${model.id}`);
  console.log(dir);
}
