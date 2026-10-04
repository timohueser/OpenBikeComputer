import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';

// A dead, hung or garbled runtime cannot recover in place: `onFailure` runs once and the
// owner restarts the whole process.
export function parserProcess(python, model, onFailure) {
  const child = spawn(python, ['query/runtime.py', '--model', model], {
    cwd: import.meta.dirname,
    stdio: ['pipe', 'pipe', 'inherit'],
  });
  const pending = new Map();
  let nextId = 0,
    available = false,
    stopped = false,
    failure = 'The query model is loading.';
  const fail = (message) => {
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
    onFailure(message);
  };
  child.on('error', () =>
    fail('The query runtime could not start. Run the local setup command.'),
  );
  child.stdin.on('error', () => fail('The query runtime input closed.'));
  child.on('exit', () => fail('The query runtime stopped.'));
  createInterface({ input: child.stdout }).on('line', (line) => {
    let value;
    try {
      value = JSON.parse(line);
    } catch {
      fail('The query runtime returned invalid data.');
      return;
    }
    if (value.ready) {
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
    status: () => ({ ready: available, message: failure }),
    parse(text) {
      if (!available) return Promise.reject(new Error(failure));
      if (pending.size >= 8)
        return Promise.reject(new Error('The query model is busy. Try again.'));
      return new Promise((resolve, reject) => {
        const id = ++nextId,
          timer = setTimeout(() => fail('The query model timed out. Try again.'), 10000);
        pending.set(id, { resolve, reject, timer });
        child.stdin.write(JSON.stringify({ id, text }) + '\n');
      });
    },
    close() {
      stopped = true;
      child.kill();
    },
  };
}
