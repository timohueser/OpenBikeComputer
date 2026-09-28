import { spawn } from 'node:child_process';
import path from 'node:path';

const root = import.meta.dirname;
const children = [
  spawn(process.execPath, ['server.mjs'], { cwd: root, stdio: 'inherit' }),
  spawn(
    'npm',
    [
      'run',
      'dev',
      '--',
      '--mode',
      'web',
      '--host',
      '127.0.0.1',
      '--port',
      process.env.OBC_PLANNER_PORT || '4184',
    ],
    { cwd: path.resolve(root, '../../builder/app'), stdio: 'inherit' },
  ),
];
let stopping = false;
function stop(code = 0) {
  if (stopping) return;
  stopping = true;
  for (const p of children) p.kill('SIGTERM');
  process.exitCode = code;
}
for (const p of children) {
  p.on('error', (error) => {
    console.error(error.message);
    stop(1);
  });
  p.on('exit', (code) => stop(code || 0));
}
process.on('SIGINT', () => stop());
process.on('SIGTERM', () => stop());
console.log(
  `Planner: http://127.0.0.1:${process.env.OBC_PLANNER_PORT || '4184'}/planner.html`,
);
