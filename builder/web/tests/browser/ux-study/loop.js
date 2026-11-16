// One walkthrough: shows the participant each screen, performs its action, and decides when the
// walkthrough ends. It knows no platform; the environment does.
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const TRIES = 3;
const STUCK_REPEATS = 3;

/** The cost of the participant calls of one study. At `maxCostUsd` no further call starts. */
export function createBudget(model, maxCostUsd) {
  let spent = 0;
  return {
    add(usage) {
      spent += (usage.input_tokens * model.usdPerMillionInput + usage.output_tokens * model.usdPerMillionOutput) / 1e6;
    },
    get spent() { return spent; },
    get exhausted() { return spent >= maxCostUsd; },
  };
}

/**
 * Runs `task` once in a new walkthrough of `env` and writes `steps.jsonl` and one PNG for each screen to `dir`.
 * Resolves with `{ outcome, steps }`: `done`, `stuck`, `gave_up` or `error`; an `error` also has a `reason`.
 */
export async function walk({ task, env, participant, budget, dir }) {
  mkdirSync(dir, { recursive: true });
  const records = [];
  let screens = 0;
  const save = png => writeFileSync(join(dir, `${String(++screens).padStart(2, '0')}.png`), png);
  const end = (outcome, reason) => {
    writeFileSync(join(dir, 'steps.jsonl'), records.map(r => `${JSON.stringify(r)}\n`).join(''));
    return { outcome, steps: records.length, ...(reason && { reason }) };
  };
  /** The value of a participant call with two more tries, or the reason that it failed. */
  const call = async request => {
    let reason;
    for (let i = 0; i < TRIES; i++) {
      if (budget.exhausted) return { reason: 'spending limit' };
      try {
        const { value, usage } = await request();
        budget.add(usage);
        return { value };
      } catch (error) {
        if (error.usage) budget.add(error.usage);
        reason = error.message;
      }
    }
    return { reason };
  };

  const walkthrough = await env.start();
  try {
    let shot = await walkthrough.screenshot();
    save(shot.png);
    let previous = null;
    for (;;) {
      const answer = await call(() => participant.step({ goal: task.goal, records, previous, current: shot.png }));
      if (!answer.value) return end('error', answer.reason);
      const { action } = answer.value;
      const where = action.type === 'give_up' ? { screen: null, hit: null } : await walkthrough.describe(action);
      const record = { step: records.length + 1, ...answer.value, ...where, unstable: shot.unstable };
      records.push(record);
      if (action.type === 'give_up') return end('gave_up');

      await walkthrough.perform(action);
      previous = shot.png;
      shot = await walkthrough.screenshot();
      save(shot.png);

      if (await task.success(walkthrough.app)) {
        const done = await call(() => participant.done({ goal: task.goal, records, current: shot.png }));
        if (!done.value) return end('error', done.reason);
        record.done_answer = done.value;
        return end('done');
      }
      if (stuck(records, env.actions.same)) return end('stuck');
      if (records.length >= task.stepBudget) return end('gave_up');
    }
  } finally {
    await walkthrough.end();
  }
}

/** The same action on the same screen in each of the last three steps. */
function stuck(records, same) {
  const last = records.slice(-STUCK_REPEATS);
  return last.length === STUCK_REPEATS
    && last.every(r => r.screen === last[0].screen && same(r.action, last[0].action));
}
