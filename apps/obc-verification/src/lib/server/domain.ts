import { MANUAL_REASONS, type Attachment, type Candidate, type ManualReason, type Readiness, type Requirement, type TestResult } from '../types.ts';
import { citedTests, coverageIssues } from '../coverage.ts';

export class Problem extends Error {
  status: number;
  constructor(status: number, message: string) { super(message); this.status = status; }
}
export function assert(value: unknown, message: string, status = 400): asserts value {
  if (!value) throw new Problem(status, message);
}
export function text(value: unknown, label: string, max = 10000): string {
  assert(typeof value === 'string' && value.trim().length > 0 && value.length <= max, `${label} is required (maximum ${max} characters).`);
  return value.trim();
}
export function identifier(value: unknown, label = 'ID'): string {
  const result = text(value, label, 200);
  assert(/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(result), `${label} contains unsupported characters.`);
  return result;
}
export function positive(value: unknown, label: string): number {
  assert(typeof value === 'number' && Number.isSafeInteger(value) && value > 0, `${label} must be a positive integer.`);
  return value;
}
export function attachments(value: unknown, lookup: (id: string) => Attachment): Attachment[] {
  assert(Array.isArray(value) && value.length <= 30, 'At most 30 attachments are allowed.');
  const seen = new Set<string>();
  return value.map((entry) => {
    assert(entry && typeof entry === 'object', 'Invalid attachment.');
    const found = lookup(identifier(entry.id));
    assert(!seen.has(found.id), 'Duplicate attachment.'); seen.add(found.id);
    return found;
  });
}
export function requirements(value: unknown, lookup: (id: string) => Attachment): Requirement[] {
  assert(Array.isArray(value) && value.length <= 1000, 'Requirements must be a list of at most 1000 entries.');
  const ids = new Set<string>();
  return value.map((r) => {
    assert(r && typeof r === 'object', 'Invalid requirement.');
    const id = identifier(r.id, 'Requirement ID');
    assert(!ids.has(id), 'Requirement IDs must be unique.'); ids.add(id);
    assert(typeof r.active === 'boolean', 'Active must be a boolean.');
    assert(Array.isArray(r.tests) && r.tests.length <= 100, 'At most 100 tests per requirement.');
    assert(r.group === undefined || (typeof r.group === 'string' && r.group.length <= 80), 'Group must be a name of at most 80 characters.');
    const group = r.group?.trim();
    assert(!group || !/[\u0000-\u001f\u007f]/.test(group), 'Group must be a single line.');
    assert(r.todo === undefined || typeof r.todo === 'boolean', 'Definition status must be a boolean.');
    assert(r.implementationNeeded === undefined || typeof r.implementationNeeded === 'boolean', 'Implementation status must be a boolean.');
    const tests = new Set<string>();
    return { id, title: text(r.title, 'Title', 300), statement: text(r.statement, 'Statement', 50000), ...(group ? { group } : {}), ...(r.todo ? { todo: true } : {}), ...(r.implementationNeeded ? { implementationNeeded: true } : {}), active: r.active,
      tests: r.tests.map((t: Record<string, unknown>) => {
        const testId = identifier(t.id, 'Test ID');
        assert(!tests.has(testId), 'Test IDs must be unique within a requirement.'); tests.add(testId);
        assert(t.kind === 'manual' || t.kind === 'automated', 'Unknown test kind.');
        assert(t.manualReason === undefined || (t.kind === 'manual' && MANUAL_REASONS.includes(t.manualReason as ManualReason)),
          `Only a manual test has a type, and it must be one of: ${MANUAL_REASONS.join(', ')}.`);
        return { id: testId, kind: t.kind, title: text(t.title, 'Test title', 300), inputs: attachments(t.inputs, lookup),
          ...(t.kind === 'manual'
            ? { steps: text(t.steps, 'Steps', 50000), expected: text(t.expected, 'Expected outcome', 50000),
                ...(t.manualReason ? { manualReason: t.manualReason as ManualReason } : {}) }
            : { caseId: text(t.caseId, 'Automated case ID', 1000) }) };
      }) };
  });
}
export function testResults(value: unknown): TestResult[] {
  assert(Array.isArray(value) && value.length <= 100000, 'Invalid result list.');
  const seen = new Set<string>();
  return value.map((r) => {
    const caseId = text(r.caseId, 'Case ID', 1000);
    assert(!seen.has(caseId), `Duplicate result: ${caseId}`); seen.add(caseId);
    assert(['pass', 'fail', 'skip', 'error'].includes(r.status), 'Invalid result status.');
    return { caseId, status: r.status, ...(r.detail ? { detail: text(r.detail, 'Result detail', 20000) } : {}) };
  });
}
/** The plan decides what must pass: only the tests it cites are evidence, even in an older candidate snapshot. */
export function requirementIssues(candidate: Candidate, requirement: Requirement, results = new Map(candidate.results.map((r) => [r.caseId, r.status]))): string[] {
  const missing: string[] = [];
  const tests = citedTests(requirement);
  if (requirement.todo) missing.push(`${requirement.id}: definition is incomplete.`);
  if (requirement.implementationNeeded) missing.push(`${requirement.id}: implementation is incomplete.`);
  if (!tests.length) missing.push(`${requirement.id}: no verification defined.`);
  missing.push(...coverageIssues(requirement).map(issue => `${requirement.id}: ${issue}`));
  for (const test of tests) {
    if (test.kind === 'automated') {
      if (!test.caseId || results.get(test.caseId) !== 'pass') missing.push(`${requirement.id} / ${test.title}: automated pass required.`);
    } else {
      const latest = candidate.manualRuns.findLast((r) => r.requirementId === requirement.id && r.testId === test.id);
      if (latest?.result !== 'pass') missing.push(`${requirement.id} / ${test.title}: manual pass required.`);
    }
  }
  return missing;
}
export function readiness(candidate: Candidate): Readiness {
  const missing: string[] = [];
  const active = candidate.revision.requirements.filter((r) => r.active);
  let verified = 0;
  let excepted = 0;
  if (!active.length) missing.push('No active requirements.');
  if (candidate.ciStatus !== 'success') missing.push('Automated release checks have not passed.');
  for (const name of ['UPDATE.BIN', 'manifest.json', 'SHA256SUMS.txt', 'obc-boot.elf', 'obc-fw-nrf54l.elf']) {
    if (!candidate.assets.some((a) => a.name === name)) missing.push(`Missing release asset: ${name}`);
  }
  const results = new Map(candidate.results.map((r) => [r.caseId, r.status]));
  for (const requirement of active) {
    const issues = requirementIssues(candidate, requirement, results);
    if (!requirement.todo && candidate.exceptions?.some((exception) => exception.requirementId === requirement.id)) excepted++;
    else if (!issues.length) verified++;
    else missing.push(...issues);
  }
  return { ready: missing.length === 0, missing, verified, total: active.length, excepted, excluded: candidate.revision.requirements.length - active.length };
}
export function refresh(candidate: Candidate): Candidate {
  if (candidate.status !== 'publishing' && candidate.status !== 'published') {
    candidate.status = candidate.ciStatus === 'failure' ? 'failed' : readiness(candidate).ready ? 'ready' : candidate.ciStatus === 'pending' && candidate.status !== 'running' ? 'queued' : 'running';
  }
  return candidate;
}
