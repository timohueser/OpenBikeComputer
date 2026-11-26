// One call to the model with images and a JSON schema for the answer, shared by the participant and the report.
import Anthropic from '@anthropic-ai/sdk';

/** A call that gave no usable answer. `usage` is what the call cost, if it reached the model. */
export class ModelError extends Error {
  constructor(message, usage = null) { super(message); this.usage = usage; }
}

export const image = jpeg => ({ type: 'image', source: { type: 'base64', media_type: 'image/jpeg', data: jpeg.toString('base64') } });

/**
 * `ask(content, schema)` sends one user message to `model` and resolves with `{ value, usage }`, where
 * `value` is the answer parsed from JSON; it throws a `ModelError`.
 */
export function createModelCall({ model, system, effort, apiKey = process.env.OBC_UX_API_KEY }) {
  if (!apiKey) throw new Error('OBC_UX_API_KEY is not set');
  const client = new Anthropic({ apiKey });
  return async (content, schema) => {
    const response = await client.messages.create({
      model: model.id,
      max_tokens: 16_000,
      system,
      messages: [{ role: 'user', content }],
      output_config: { effort, format: { type: 'json_schema', schema } },
    }).catch(error => { throw new ModelError(error.message); });
    const { usage } = response;
    if (response.stop_reason !== 'end_turn') throw new ModelError(`stop_reason ${response.stop_reason}`, usage);
    const text = response.content.find(block => block.type === 'text')?.text;
    try {
      return { value: JSON.parse(text), usage };
    } catch {
      throw new ModelError('the answer is not JSON', usage);
    }
  };
}
