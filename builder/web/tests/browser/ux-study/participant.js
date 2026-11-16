// The participant: one call to the model for each step, which answers with a step record.
import { readFileSync } from 'node:fs';
import { createModelCall, image, ModelError } from './model.js';

const PROMPT = readFileSync(new URL('./participant.md', import.meta.url), 'utf8');

const stepSchema = actionSchema => ({
  type: 'object',
  properties: {
    options: { type: 'array', items: { type: 'string' } },
    expect: { type: 'string' },
    action: actionSchema,
    match: { anyOf: [{ type: 'string', enum: ['yes', 'partly', 'no'] }, { type: 'null' }] },
    confusion: {
      type: 'object',
      properties: { level: { type: 'integer', enum: [0, 1, 2] }, reason: { type: 'string' } },
      required: ['level', 'reason'],
      additionalProperties: false,
    },
  },
  required: ['options', 'expect', 'action', 'match', 'confusion'],
  additionalProperties: false,
});

const doneSchema = {
  type: 'object',
  properties: { answer: { type: 'string', enum: ['yes', 'no'] } },
  required: ['answer'],
  additionalProperties: false,
};

/**
 * The newcomer on `model`, acting with the `actions` of an environment. Each method resolves with
 * `{ value, usage }` or throws a `ModelError`.
 */
export function createParticipant({ model, actions, apiKey }) {
  const ask = createModelCall({ model, system: `${PROMPT}\n${actions.text}`, effort: 'low', apiKey });
  const schema = stepSchema(actions.schema);

  const history = (goal, records) => [
    { type: 'text', text: `Your goal: ${goal}` },
    { type: 'text', text: records.length ? `Your earlier steps:\n${records.map(r => JSON.stringify(r)).join('\n')}` : 'This is your first step.' },
  ];

  return {
    /** The step record for the current screenshot. */
    async step({ goal, records, previous, current }) {
      const content = [...history(goal, records)];
      if (previous) content.push({ type: 'text', text: 'The screen before your last action:' }, image(previous));
      content.push({ type: 'text', text: 'The screen now:' }, image(current));
      const { value, usage } = await ask(content, schema);
      if (!fits(value)) throw new ModelError('the record does not fit the schema', usage);
      return { value, usage };
    },
    /** `yes` or `no`: does the participant think that the task is done? */
    async done({ goal, records, current }) {
      const content = [...history(goal, records), { type: 'text', text: 'The screen now:' }, image(current),
        { type: 'text', text: 'Is your task done?' }];
      const { value, usage } = await ask(content, doneSchema);
      if (value?.answer !== 'yes' && value?.answer !== 'no') throw new ModelError('the answer does not fit the schema', usage);
      return { value: value.answer, usage };
    },
  };
}

/** A minimal check of the fields the run loop reads; the API holds the rest of the schema. */
const fits = record => Array.isArray(record?.options) && typeof record.expect === 'string'
  && typeof record.action?.type === 'string' && [0, 1, 2].includes(record.confusion?.level);
