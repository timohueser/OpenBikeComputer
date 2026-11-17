// One walkthrough: shows the participant each screen, performs its action, and decides when the
// walkthrough ends. It knows no platform; the environment does.
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const TRIES = 3;
const STUCK_REPEATS = 3;

/**
 * The cost of the model calls of one study. At `maxCostUsd` no further call starts. Calls that are already in
 * flight still finish, so a study can pass the limit by the cost of one call for each walkthrough that runs at
 * the same time.
 */
export function createBudget(model, maxCostUsd) {
  let spent = 0;
  return {
    add(usage) { spent += (usage.input_tokens * model.usdPerMillionInput + usage.output_tokens * model.usdPerMillionOutput) / 1e6; },
    get spent() { return spent; },
    get exhausted() { return spent >= maxCostUsd; },
  };
}

/**
 * Runs `task` once in a new walkthrough of `env` and writes `steps.jsonl` and one JPEG for each screen to `dir`.
 * Resolves with `{ outcome, steps }`: `done`, `stuck`, `gave_up` or `error`; an `error` also has a `reason`.
 * After any other outcome, the participant names its hardest step, which goes into that step's record as
 * `hardest`. The participant gets only its own answers back: the fields that the loop adds come from the
 * page, which the participant must know only from the screenshots.
 */
export async function walk({ task, env, participant, budget, dir }) {
  mkdirSync(dir, { recursive: true });
  const answers = [];
  const records = [];
  let screens = 0;
  const save = jpeg => writeFileSync(join(dir, `${String(++screens).padStart(2, '0')}.jpg`), jpeg);
  const end = (outcome, reason) => ({ outcome, steps: records.length, ...(reason && { reason }) });
  /** The value of a participant call with two more tries, or the reason that it failed. */
  const call = async (request, invalid = () => null) => {
    let reason;
    for (let i = 0; i < TRIES; i++) {
      if (budget.exhausted) return { reason: 'spending limit' };
      try {
        const { value, usage } = await request();
        budget.add(usage);
        reason = invalid(value);
        if (!reason) return { value };
      } catch (error) {
        if (error.usage) budget.add(error.usage);
        reason = error.message;
      }
    }
    return { reason };
  };
  const invalidAction = answer => (env.actions.valid(answer.action) ? null : `invalid action ${JSON.stringify(answer.action)}`);
  const invalidStep = ({ step }) => (step === null || (step >= 1 && step <= records.length) ? null : `no step ${step}`);
  let shot;
  /** Ends with `outcome`; a failed end question leaves the outcome as it is. */
  const finish = async outcome => {
    const hardest = await call(() => participant.hardest({ goal: task.goal, records: answers, current: shot.jpeg }), invalidStep);
    if (hardest.value?.step) records[hardest.value.step - 1].hardest = hardest.value.why;
    return end(outcome);
  };

  let walkthrough;
  try {
    walkthrough = await env.start();
    shot = await walkthrough.screenshot();
    save(shot.jpeg);
    let previous = null;
    for (;;) {
      const step = { goal: task.goal, records: answers, previous, current: shot.jpeg };
      const answer = await call(() => participant.step(step), invalidAction);
      if (!answer.value) return end('error', answer.reason);
      const { action } = answer.value;
      const where = await walkthrough.describe(action);
      if (action.type === 'give_up') where.hit = null;
      const record = { step: records.length + 1, ...answer.value, ...where, unstable: shot.unstable };
      answers.push(answer.value);
      records.push(record);
      if (action.type === 'give_up') return await finish('gave_up');

      await walkthrough.perform(action);
      previous = shot.jpeg;
      shot = await walkthrough.screenshot();
      save(shot.jpeg);

      if (await task.success(walkthrough.app)) {
        const done = await call(() => participant.done({ goal: task.goal, records: answers, current: shot.jpeg }));
        if (!done.value) return end('error', done.reason);
        record.done_answer = done.value;
        return await finish('done');
      }
      if (stuck(records, env.actions.same)) return await finish('stuck');
      if (records.length >= task.stepBudget) return await finish('gave_up');
    }
  } finally {
    writeFileSync(join(dir, 'steps.jsonl'), records.map(r => `${JSON.stringify(r)}\n`).join(''));
    await walkthrough?.end();
  }
}

/** The same action on the same screen in each of the last three steps. */
function stuck(records, same) {
  const last = records.slice(-STUCK_REPEATS);
  return last.length === STUCK_REPEATS
    && last.every(r => r.screen === last[0].screen && same(r.action, last[0].action));
}
