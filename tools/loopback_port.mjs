// The loopback port of a Playwright suite's static server, so that suites in other worktrees run at
// the same time.
import { createServer } from 'node:net';

/**
 * The port in the environment variable `name`, else a free one that the kernel picks. Playwright
 * loads the config again in each worker, and a worker inherits the environment of the runner, so
 * the port that the runner picks holds in every worker.
 */
export async function loopbackPort(name) {
  if (!process.env[name]) {
    const server = createServer();
    await new Promise((resolve, reject) => server.once('error', reject).listen(0, '127.0.0.1', resolve));
    process.env[name] = String(server.address().port);
    await new Promise(resolve => server.close(resolve));
  }
  const port = Number(process.env[name]);
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    throw new Error(`${name} must be an integer from 1 to 65535`);
  }
  return port;
}
