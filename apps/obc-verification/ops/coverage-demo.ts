import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { randomBytes, scryptSync } from 'node:crypto';
import { spawn, execFileSync } from 'node:child_process';
import type { RequestEvent } from '@sveltejs/kit';
import type { Candidate, CoveragePlan, Requirement, VerificationTest } from '../src/lib/types.ts';

const directory = mkdtempSync(join(tmpdir(), 'obc-coverage-demo-'));
const port = Number(process.env.VERIFICATION_DEMO_PORT || 4180);
if (!Number.isInteger(port) || port < 1024 || port > 65535) throw new Error('Choose a local port from 1024 to 65535.');
process.env.ORIGIN = `http://127.0.0.1:${port}`;
process.env.VERIFICATION_DATA_DIR = directory;
process.env.VERIFICATION_DEMO = '1';
process.env.VERIFICATION_OWNER_USERNAME = 'demo';
const password = 'local-coverage-demo';
const salt = randomBytes(16).toString('hex');
process.env.VERIFICATION_OWNER_PASSWORD_HASH = `${salt}:${scryptSync(password, salt, 64).toString('hex')}`;
for (const key of ['GITHUB_TOKEN', 'GITHUB_CLIENT_ID', 'GITHUB_CLIENT_SECRET', 'VERIFICATION_CI_TOKEN']) delete process.env[key];
const { store } = await import('../src/lib/server/store.ts');
const { api } = await import('../src/lib/server/api.ts');
const { authenticate, createAgentToken } = await import('../src/lib/server/auth.ts');
const db = store();
const sourceSha = execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
const owner = { name: 'demo', role: 'owner' as const, provider: 'local' as const, userId: 'local', admin: true };
const cases = [
  { id: 'rust-test::obc-app::main::app::north_up_is_the_default_orientation', suite: 'rust-test', name: 'North-up projection' },
  { id: 'rust-test::obc-app::main::app::heading_up_rotates_course_to_screen_top', suite: 'rust-test', name: 'Heading-up projection' },
  { id: 'rust-test::obc-app::harness::screens::pan_back_exits_and_recenters', suite: 'rust-test', name: 'Back exits pan and recenters' },
  { id: 'demo::orientation-selection', suite: 'Illustrative only — not an implemented test', name: 'DEMO ONLY: choosing an orientation' },
  { id: 'demo::old-smoke-test', suite: 'Illustrative only — not an implemented test', name: 'DEMO ONLY: obsolete map smoke test' }
];
db.put('catalog', 'current', { sourceSha, updatedAt: new Date().toISOString(), cases });
const requirements: Requirement[] = [
  { id: 'SYS-039', title: 'Map orientation', statement: 'The user shall be able to choose heading-up or north-up map orientation. The choice shall be persistent through restarts.', group: 'Map display on device', active: true, tests: [] },
  { id: 'SYS-030', title: 'Return to position', statement: 'The user shall be able to exit pan/zoom mode with one action. This shall restore following mode and center the map back on the users position given a GPS fix is available.', group: 'Map display on device', active: true,
    tests: [{ id: 'manual-return', kind: 'manual', title: 'DEMO ONLY: ride check for return to position', steps: '1. Start a ride with a position fix.\n2. Pan the map away from the position.\n3. Use the Back action once.', expected: 'The map follows the position again and centers on it.', inputs: [] }] }
];
const plans: CoveragePlan[] = [
  { rationale: 'Projection tests cover both render modes. They do not prove that a user can select a mode or retain that choice after restart.', criteria: [
    { id: 'north-up', statement: 'The map can render north-up.', evidence: [{ caseId: cases[0].id, rationale: 'Checks the default viewport angle and projection of a point due north.' }], gap: '' },
    { id: 'heading-up', statement: 'The map can render heading-up.', evidence: [{ caseId: cases[1].id, rationale: 'Checks that the current course projects toward the top of the screen.' }], gap: '' },
    { id: 'selection', statement: 'The user can select either orientation.', evidence: [{ caseId: cases[4].id, rationale: 'Illustrative assertion: an old smoke test that only opens the map. It does not exercise the orientation control.' }], gap: 'Replace the smoke test with a real user-interaction test for both choices.', next: { level: 'system', summary: 'Drive the orientation control in the simulator and assert the map rotates for both choices.' } },
    { id: 'persistence', statement: 'The selected orientation survives a restart.', evidence: [], gap: 'Add a save/reload test that also starts a ride and checks that it respects the saved choice.' }
  ] },
  { rationale: 'The shared application test exercises the single Back action, the resulting Follow mode, and recentering on a valid position fix. A ride check confirms the same behavior on the device.', criteria: [
    { id: 'return', statement: 'One action exits pan/zoom, restores following, and recenters on the available fix.', evidence: [
      { caseId: cases[2].id, rationale: 'Runs the Back gesture and asserts the mode and camera position afterwards.' },
      { testId: 'manual-return', rationale: 'Confirms the same behavior on the device, with a real position fix.' }
    ], gap: '' }
  ] }
];
const credential = createAgentToken(owner, 'Local coverage demo agent', new Date(Date.now() + 86_400_000).toISOString()).token;
const cookies = { get: () => undefined } as unknown as RequestEvent['cookies'];
async function propose(index: number, procedures: VerificationTest[] = []) {
  const request = new Request(`${process.env.ORIGIN}/api/coverage-proposals`, { method: 'POST', headers: { authorization: `Bearer ${credential}`, 'content-type': 'application/json' },
    body: JSON.stringify({ baseRevision: db.latestRevision().id, requirementId: requirements[index].id, sourceSha, plan: plans[index], ...(procedures.length ? { procedures } : {}) }) });
  const response = await api({ request, params: { path: 'coverage-proposals' }, locals: { actor: authenticate(request, cookies) }, cookies } as unknown as RequestEvent);
  if (!response.ok) throw new Error(await response.text());
  return await response.json();
}
db.saveRevision(db.latestRevision().id, 'Local demo setup', requirements);
for (let i = 0; i < plans.length; i++) db.decideCoverageProposal((await propose(i)).id, 'Simulated owner review', true);
const candidate: Candidate = { id: 'coverage-demo', version: 'v0.0.0-coverage-demo', sourceRef: 'Local demonstration — simulated results', sourceSha, revision: db.latestRevision(), createdAt: new Date().toISOString(), status: 'running', ciStatus: 'success',
  results: cases.map(c => ({ caseId: c.id, status: 'pass', detail: 'Simulated result for the local coverage demonstration. This is not release evidence.' })), manualRuns: [], assets: [] };
db.put('candidate', candidate.id, candidate);
// An agent replaces obsolete evidence in an accepted partial plan. The omitted test is unlinked on approval.
plans[0].criteria[2].evidence = [{ caseId: cases[3].id, rationale: 'Illustrative assertion: choose each orientation through the control and inspect the map. This test is invented for the demo, not production evidence.' }];
plans[0].criteria[2].gap = '';
delete plans[0].criteria[2].next;
plans[0].criteria[3].evidence = [{ testId: 'ride-restart', rationale: 'Confirms on the device that the choice is still active after a power cycle.' }];
plans[0].criteria[3].gap = 'No automated check yet; the ride check covers it until one exists.';
plans[0].criteria[3].next = { level: 'unit', summary: 'Persist the choice to the settings store, reload, and assert the stored value.' };
plans[0].rationale = 'Demo revision: replace the obsolete smoke test with evidence for user selection and add a ride check for persistence. Coverage stays partial.';
await propose(0, [{ id: 'ride-restart', kind: 'manual', title: 'DEMO ONLY: ride check after restart', steps: '1. Set heading-up.\n2. Power the device off and on.\n3. Start a ride.', expected: 'The map stays heading-up.', inputs: [] }]);
// A requirement edit keeps its plan and is approved by the save; the candidate above stays frozen.
const changed = db.latestRevision();
changed.requirements[1].statement += ' Returning shall also preserve the selected zoom level. (Demo addition.)';
db.saveRevision(changed.id, 'Demo requirement edit', changed.requirements);
plans[1].criteria.push({ id: 'zoom', statement: 'Returning preserves the selected zoom level.', evidence: [], gap: 'Add an assertion for the new zoom-preservation obligation. This obligation is added only in the demo.' });
plans[1].rationale = 'The requirement gained a zoom-preservation clause. Keep the existing Back-action evidence and record the new gap.';
await propose(1);
db.db.close();
const child = spawn(process.execPath, ['node_modules/vite/bin/vite.js', 'dev', '--host', '127.0.0.1', '--port', String(port), '--strictPort'], { cwd: resolve(import.meta.dirname, '..'), env: process.env, stdio: 'inherit' });
console.log(`\nLocal coverage demo: ${process.env.ORIGIN}\nUsername: demo\nPassword: ${password}\nDisposable database: ${directory}\nSYS-039: approve or edit the proposed evidence replacement. SYS-030: a proposal records a demo requirement addition as a gap. The release candidate retains the earlier snapshot. New demo tests and all results are illustrative.\n`);
for (const signal of ['SIGINT', 'SIGTERM'] as const) process.on(signal, () => child.kill(signal));
child.on('exit', code => { rmSync(directory, { recursive: true, force: true }); process.exitCode = code ?? 0; });
