import assert from 'node:assert/strict';
import { test } from 'node:test';
import { run } from './run.js';

test('an unknown model stops the study before it starts, and the error lists the known models', async () => {
  await assert.rejects(run({ modelId: 'claude-sonnet' }),
    { message: "no model 'claude-sonnet'; the models are: claude-sonnet-5-5, claude-opus-5-5, claude-haiku-5-5" });
});
