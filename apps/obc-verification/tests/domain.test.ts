import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { readiness, requirements, testResults } from '../src/lib/server/domain.ts';
import type { Candidate } from '../src/lib/types.ts';
export function candidate(): Candidate {
  return { id: 'candidate', version: '0.1.0', sourceRef: 'develop', sourceSha: 'a'.repeat(40), createdAt: '', status: 'running', ciStatus: 'success',
    revision: { id: 1, author: 'owner', createdAt: '', requirements: [{ id: 'REQ-1', title: 'Recover data', statement: '**Required**', active: true, coverage: { rationale: 'Both recovery obligations have evidence.', criteria: [{ id: 'recover', statement: 'Data is retained through a power cut.', evidence: [{ caseId: 'suite::class::recovery', rationale: 'Automated recovery path.' }, { testId: 'manual', rationale: 'Physical power cut.' }], gap: '' }], review: { author: 'owner', createdAt: '', sourceSha: 'a'.repeat(40) } }, tests: [{ id: 'auto', title: 'Recovery', kind: 'automated', caseId: 'suite::class::recovery', inputs: [] }, { id: 'manual', title: 'Power cut', kind: 'manual', steps: 'Cut power', expected: 'Data retained', inputs: [] }] }] },
    results: [{ caseId: 'suite::class::recovery', status: 'pass' }], manualRuns: [{ id: 'run', requirementId: 'REQ-1', testId: 'manual', result: 'pass', device: 'board 1', notes: '', evidence: [], author: 'owner', createdAt: '' }], assets: ['UPDATE.BIN', 'manifest.json', 'SHA256SUMS.txt', 'obc-boot.elf', 'obc-fw-nrf54l.elf'].map((name) => ({ id: name, name, size: 1, sha256: 'a'.repeat(64) })) };
}
test('release gate fails closed for missing, skipped, obsolete and partial evidence', () => {
  const c = candidate(); assert.equal(readiness(c).ready, true);
  c.revision.requirements[0].group = 'Navigation'; assert.equal(readiness(c).ready, true);
  c.results[0].status = 'skip'; assert.equal(readiness(c).ready, false);
  c.results[0].status = 'pass'; c.manualRuns.push({ ...c.manualRuns[0], id: 'new-run', result: 'blocked' }); assert.equal(readiness(c).ready, false);
  c.manualRuns.pop(); c.assets.pop(); assert.equal(readiness(c).ready, false);
  c.assets = candidate().assets; c.ciStatus = 'failure'; assert.equal(readiness(c).ready, false);
  c.ciStatus = 'success'; c.revision.requirements.push({ id: 'REQ-2', title: 'New', statement: 'Required', active: true, tests: [] }); assert.equal(readiness(c).ready, false);
  c.revision.requirements = []; assert.equal(readiness(c).ready, false);
});
test('definitions preserve Markdown and reject ambiguous identities or forged attachments', () => {
  const reqs = candidate().revision.requirements;
  assert.equal(requirements(reqs, () => { throw new Error('unknown'); })[0].statement, '**Required**');
  assert.throws(() => requirements([...reqs, ...reqs], () => { throw new Error('unknown'); }), /unique/);
  assert.throws(() => testResults([{ caseId: 'same', status: 'pass' }, { caseId: 'same', status: 'fail' }]), /Duplicate/);
  reqs[0].tests[0].inputs = [{ id: 'forged', name: 'ignored', sha256: '', size: 0 }];
  assert.throws(() => requirements(reqs, () => { throw new Error('unknown file'); }), /unknown file/);
});

test('requirements accept one optional flat group and normalize ungrouped values', () => {
  const req = candidate().revision.requirements[0];
  const read = (group: unknown) => requirements([{ ...req, group }], () => { throw new Error('unknown file'); })[0];
  assert.equal(read('  Navigation  ').group, 'Navigation');
  assert.equal(read(undefined).group, undefined);
  assert.equal(read('   ').group, undefined);
  assert.throws(() => read({ name: 'Navigation', parent: 'Device' }), /Group/);
  assert.throws(() => read(['Navigation', 'Bluetooth']), /Group/);
  assert.throws(() => read('x'.repeat(81)), /80/);
  assert.throws(() => read('Navigation\nRoutes'), /single line/);
});

test('active unfinished definitions cannot be excepted and to-do status is a strict optional boolean', () => {
  const c = candidate();
  const req = c.revision.requirements[0];
  const read = (todo: unknown) => requirements([{ ...req, todo }], () => { throw new Error('unknown file'); })[0];
  assert.equal(read(true).todo, true);
  assert.equal(read(false).todo, undefined);
  assert.equal(read(undefined).todo, undefined);
  for (const invalid of ['true', null, 1]) assert.throws(() => read(invalid), /boolean/);
  req.todo = true;
  c.exceptions = [{ requirementId: req.id, reason: 'Cannot skip definition', author: 'owner', createdAt: '' }];
  assert.equal(readiness(c).ready, false);
  assert.equal(readiness(c).excepted, 0);
  assert.equal(readiness(c).verified, 0);
  assert.match(readiness(c).missing.join(' '), /definition is incomplete/);
  req.active = false;
  c.revision.requirements.push({ ...candidate().revision.requirements[0], id: 'REQ-2' });
  c.manualRuns.push({ ...c.manualRuns[0], requirementId: 'REQ-2' });
  assert.deepEqual(readiness(c), { ready: true, missing: [], verified: 1, total: 1, excepted: 0, excluded: 1 });
});

test('candidate exceptions remain distinct from verification and never bypass CI or firmware gates', () => {
  const c = candidate();
  const req = c.revision.requirements[0];
  c.exceptions = [{ requirementId: req.id, reason: 'Known gap outside the assertions', author: 'owner', createdAt: '' }];
  assert.deepEqual(readiness(c), { ready: true, missing: [], verified: 0, total: 1, excepted: 1, excluded: 0 });
  c.results[0].status = 'fail'; c.manualRuns[0].result = 'blocked';
  assert.equal(readiness(c).ready, true);
  c.ciStatus = 'failure'; assert.equal(readiness(c).ready, false);
  c.ciStatus = 'pending'; assert.equal(readiness(c).ready, false);
  c.ciStatus = 'success'; c.assets.pop(); assert.equal(readiness(c).ready, false);
  c.assets = candidate().assets;
  req.tests = []; assert.equal(readiness(c).ready, true);
  c.exceptions = []; assert.equal(readiness(c).ready, false);
  const next = candidate(); next.results = []; next.manualRuns = [];
  assert.equal(readiness(next).ready, false); assert.equal(readiness(next).excepted, 0);
  c.revision.requirements = []; assert.equal(readiness(c).ready, false);
});


test('implementation-needed requirements block passing evidence, allow explicit exceptions, and count exclusions separately', () => {
  const c = candidate();
  const req = c.revision.requirements[0];
  const read = (implementationNeeded: unknown) => requirements([{ ...req, implementationNeeded }], () => { throw new Error('unknown file'); })[0];
  assert.equal(read(true).implementationNeeded, true);
  assert.equal(read(false).implementationNeeded, undefined);
  assert.equal(read(undefined).implementationNeeded, undefined);
  for (const invalid of ['true', null, 1]) assert.throws(() => read(invalid), /boolean/);
  req.implementationNeeded = true;
  const incomplete = readiness(c);
  assert.equal(incomplete.ready, false); assert.equal(incomplete.verified, 0);
  assert.match(incomplete.missing.join(' '), /implementation is incomplete/);
  c.exceptions = [{ requirementId: req.id, reason: 'Accepted implementation gap', author: 'owner', createdAt: '' }];
  assert.deepEqual(readiness(c), { ready: true, missing: [], verified: 0, total: 1, excepted: 1, excluded: 0 });
  req.todo = true;
  assert.equal(readiness(c).ready, false); assert.equal(readiness(c).excepted, 0);
  req.active = false; req.tests = [];
  const excludedOnly = readiness(c);
  assert.equal(excludedOnly.ready, false); assert.equal(excludedOnly.total, 0); assert.equal(excludedOnly.excluded, 1);
  assert.deepEqual(excludedOnly.missing, ['No active requirements.']);
  const included = candidate().revision.requirements[0]; included.id = 'REQ-2';
  c.revision.requirements.push(included); c.manualRuns.push({ ...c.manualRuns[0], requirementId: included.id });
  assert.deepEqual(readiness(c), { ready: true, missing: [], verified: 1, total: 1, excepted: 0, excluded: 1 });
});
