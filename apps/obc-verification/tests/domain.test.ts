import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { readiness, requirements, testResults } from '../src/lib/server/domain.ts';
import type { Candidate } from '../src/lib/types.ts';
export function candidate(): Candidate {
  return { id: 'candidate', version: '0.1.0', sourceRef: 'develop', sourceSha: 'a'.repeat(40), createdAt: '', status: 'running', ciStatus: 'success',
    revision: { id: 1, author: 'owner', createdAt: '', requirements: [{ id: 'REQ-1', title: 'Recover data', statement: '**Required**', active: true, tests: [{ id: 'auto', title: 'Recovery', kind: 'automated', caseId: 'suite::class::recovery', inputs: [] }, { id: 'manual', title: 'Power cut', kind: 'manual', steps: 'Cut power', expected: 'Data retained', inputs: [] }] }] },
    results: [{ caseId: 'suite::class::recovery', status: 'pass' }], manualRuns: [{ id: 'run', requirementId: 'REQ-1', testId: 'manual', result: 'pass', device: 'board 1', notes: '', evidence: [], author: 'owner', createdAt: '' }], assets: ['UPDATE.BIN', 'manifest.json', 'SHA256SUMS.txt', 'obc-boot.elf', 'obc-fw-nrf54l.elf'].map((name) => ({ id: name, name, size: 1, sha256: 'a'.repeat(64) })) };
}
test('release gate fails closed for missing, skipped, obsolete and partial evidence', () => {
  const c = candidate(); assert.equal(readiness(c).ready, true);
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
