// The command of a study: `node ux-study/run.js [--task ID]` runs each task (or one) three times at once
// and writes the walkthroughs and the report to `.artifacts/ux-study/<UTC time>/`.
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { openBrowser } from './environment.js';
import { createBudget, walk } from './loop.js';
import { createParticipant } from './participant.js';
import { writeReport } from './report.js';
import { maxCostUsd, model, tasks, walkthroughsPerTask } from './study.js';

const ARTIFACTS = fileURLToPath(new URL('../../../../../.artifacts/ux-study', import.meta.url));

/**
 * Runs the study, writes its report, and resolves with its folder, the path of the report, the result of each
 * walkthrough and the cost. `participant` and `ask` replace the model calls of the walkthroughs and of the report.
 */
export async function run({ taskId, participant, ask, walkthroughs = walkthroughsPerTask } = {}) {
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
  const limited = budget.exhausted;
  const report = await writeReport({ dir, results, budget, model, maxCostUsd, limited, ask });
  return { dir, report, results, costUsd: budget.spent, limited };
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const { values } = parseArgs({ options: { task: { type: 'string' } } });
  const started = Date.now();
  const { report, results, costUsd, limited } = await run({ taskId: values.task });
  if (limited) console.log(`The spending limit of ${maxCostUsd} USD stopped the study.`);
  for (const r of results) console.log(`${r.task}-${r.n}: ${r.outcome} after ${r.steps ?? 0} steps${r.reason ? ` (${r.reason})` : ''}`);
  console.log(`${costUsd.toFixed(2)} USD, ${Math.round((Date.now() - started) / 60_000)} min, ${model.id}`);
  console.log(report);
}
