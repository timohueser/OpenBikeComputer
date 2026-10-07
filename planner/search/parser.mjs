import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';

// A dead, hung or garbled runtime cannot recover in place: `onFailure` runs once and the
// owner restarts the whole process. `close` fails pending requests without reporting.
export function parserProcess(python, model, onFailure) {
  const child = spawn(python, ['query/runtime.py', '--model', model], {
    cwd: import.meta.dirname,
    stdio: ['pipe', 'pipe', 'inherit'],
  });
  const pending = new Map();
  let nextId = 0,
    available = false,
    stopped = false,
    modelIdentity = null,
    failure = 'The query model is loading.';
  const fail = (message, report = true) => {
    if (stopped) return;
    stopped = true;
    available = false;
    failure = message;
    child.kill();
    for (const p of pending.values()) {
      clearTimeout(p.timer);
      p.reject(new Error(message));
    }
    pending.clear();
    if (report) onFailure(message);
  };
  child.on('error', () =>
    fail('The query runtime could not start. Run the local setup command.'),
  );
  child.stdin.on('error', () => fail('The query runtime input closed.'));
  child.on('exit', () => fail('The query runtime stopped.'));
  createInterface({ input: child.stdout }).on('line', (line) => {
    if (stopped) return;
    let value;
    try {
      value = JSON.parse(line);
    } catch {
      fail('The query runtime returned invalid data.');
      return;
    }
    if (value.ready) {
      const names = ['labels.json', 'tokenizer.json', 'model.int8.onnx'];
      if (!value.model || Object.keys(value.model).length !== names.length ||
          names.some(name => !/^[a-f0-9]{64}$/.test(value.model[name]))) {
        fail('The query runtime returned no model identity.');
        return;
      }
      modelIdentity = value.model;
      available = true;
      failure = '';
      return;
    }
    const p = pending.get(value.id);
    if (!p) return;
    pending.delete(value.id);
    clearTimeout(p.timer);
    if (value.error) p.reject(new Error(value.error));
    else p.resolve(value.result);
  });
  return {
    status: () => ({ ready: available, message: failure, model: modelIdentity }),
    parse(text) {
      if (!available) return Promise.reject(new Error(failure));
      if (pending.size >= 8)
        return Promise.reject(new Error('The query model is busy. Try again.'));
      return new Promise((resolve, reject) => {
        const id = ++nextId,
          // A reply can wait in the pipe while the thread is busy; the poll phase reads it first.
          timer = setTimeout(() => setImmediate(() =>
            pending.has(id) && fail('The query model timed out. Try again.')), 10000);
        pending.set(id, { resolve, reject, timer });
        child.stdin.write(JSON.stringify({ id, text }) + '\n');
      });
    },
    close: () => fail('The query model stopped.', false),
  };
}
