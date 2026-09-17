import { TEST_LEVELS, type AcceptanceCriterion, type Catalog, type CoveragePlan, type CoverageProposal, type Requirement, type Revision, type VerificationTest } from '../types.ts';
import { evidenceKey, planProblem, verificationDefinition } from '../coverage.ts';
import { assert, identifier, text } from './domain.ts';

/** Validates a plan for a requirement. Automated evidence may name any catalogue case; manual evidence names a procedure the requirement has or the proposal brings. Both are linked when the plan is saved. */
export function coveragePlan(value: unknown, requirement: Requirement, catalog: Catalog, procedures: VerificationTest[] = []): CoveragePlan {
  assert(value && typeof value === 'object', 'Coverage plan is required.');
  const plan = value as Record<string, any>;
  assert(typeof plan.rationale === 'string' && plan.rationale.length <= 10000, 'Coverage summary must be text of at most 10000 characters.');
  assert(Array.isArray(plan.criteria) && plan.criteria.length > 0 && plan.criteria.length <= 100, 'Define 1 to 100 acceptance criteria.');
  const criteriaIds = new Set<string>();
  const caseIds = new Set(catalog.cases.map(c => c.id));
  const criteria: AcceptanceCriterion[] = plan.criteria.map((entry: any) => {
    assert(entry && typeof entry === 'object', 'Invalid acceptance criterion.');
    const id = identifier(entry.id, 'Criterion ID');
    assert(!criteriaIds.has(id), 'Criterion IDs must be unique.'); criteriaIds.add(id);
    assert(Array.isArray(entry.evidence) && entry.evidence.length <= 100, 'At most 100 evidence links per criterion.');
    assert(typeof entry.gap === 'string' && entry.gap.length <= 5000, 'Describe the remaining gap, or use an empty string.');
    const seen = new Set<string>();
    const evidence = entry.evidence.map((e: any) => {
      assert(e && typeof e === 'object' && (typeof e.caseId === 'string') !== (typeof e.testId === 'string'), 'Evidence must identify either a catalogue case or an existing manual test.');
      const ref = e.caseId !== undefined ? { caseId: text(e.caseId, 'Case ID', 1000) } : { testId: identifier(e.testId, 'Manual test ID') };
      assert(ref.caseId ? caseIds.has(ref.caseId) || requirement.tests.some(t => t.caseId === ref.caseId) : requirement.tests.some(t => t.id === ref.testId && t.kind === 'manual') || procedures.some(p => p.id === ref.testId), 'Evidence test is not available.');
      const key = JSON.stringify(ref);
      assert(!seen.has(key), 'Duplicate evidence for a criterion.'); seen.add(key);
      return { ...ref, rationale: text(e.rationale, 'Evidence rationale', 5000) };
    });
    const next = entry.next === undefined || entry.next === null ? undefined : proposedTest(entry.next);
    return { id, statement: text(entry.statement, 'Acceptance criterion', 5000), evidence, gap: entry.gap.trim(), ...(next ? { next } : {}) };
  });
  const result: CoveragePlan = { rationale: plan.rationale.trim(), criteria };
  const problem = planProblem(result);
  assert(!problem, problem ?? '');
  assert(new Set(criteria.flatMap(c => c.evidence.map(evidenceKey))).size <= 100, 'At most 100 tests can be evidence for a requirement.');
  return result;
}
function proposedTest(value: unknown) {
  assert(value && typeof value === 'object', 'A proposed test needs a level and a summary.');
  const { level, summary } = value as Record<string, unknown>;
  assert(typeof level === 'string' && (TEST_LEVELS as string[]).includes(level), `Test level must be one of ${TEST_LEVELS.join(', ')}.`);
  return { level: level as (typeof TEST_LEVELS)[number], summary: text(summary, 'Proposed test', 300) };
}
/** New manual procedures a proposal brings. Each must be cited by the plan; approval creates them on the requirement. */
export function proposalProcedures(value: unknown, requirement: Requirement): VerificationTest[] {
  if (value === undefined || value === null) return [];
  assert(Array.isArray(value) && value.length <= 20, 'At most 20 new procedures per proposal.');
  const ids = new Set<string>();
  return value.map((entry: any) => {
    assert(entry && typeof entry === 'object', 'Invalid procedure.');
    const id = identifier(entry.id, 'Procedure ID');
    assert(!ids.has(id) && !requirement.tests.some(t => t.id === id), 'Procedure IDs must be new and unique.'); ids.add(id);
    assert(entry.kind === undefined || entry.kind === 'manual', 'A proposed procedure is a manual test.');
    return { id, kind: 'manual' as const, title: text(entry.title, 'Procedure title', 300), steps: text(entry.steps, 'Steps', 50000), expected: text(entry.expected, 'Expected outcome', 50000), inputs: [] };
  });
}
export function commitSha(value: unknown): string {
  assert(typeof value === 'string' && /^[a-f0-9]{40}$/.test(value), 'Use the exact 40-character source commit that you assessed.');
  return value;
}
/** Makes the requirement's tests exactly the tests its plan cites: links new catalogue cases, adds the proposal's procedures, and drops the rest. */
export function linkEvidence(requirement: Requirement, plan: CoveragePlan, catalog: Catalog, id: () => string, procedures: VerificationTest[] = []): void {
  const cited = new Set(plan.criteria.flatMap(c => c.evidence.map(evidenceKey)));
  for (const evidence of plan.criteria.flatMap(c => c.evidence)) {
    if (evidence.caseId && !requirement.tests.some(t => t.caseId === evidence.caseId)) {
      const found = catalog.cases.find(c => c.id === evidence.caseId)!;
      requirement.tests.push({ id: id(), title: found.name.slice(0, 300), kind: 'automated', caseId: found.id, inputs: [] });
    }
    const procedure = evidence.testId && !requirement.tests.some(t => t.id === evidence.testId) ? procedures.find(p => p.id === evidence.testId) : undefined;
    if (procedure) requirement.tests.push(structuredClone(procedure));
  }
  requirement.tests = requirement.tests.filter(t => cited.has(t.kind === 'automated' ? t.caseId! : t.id));
}

/** Why a proposal cannot be approved: its requirement is gone or its evidence is no longer available. */
export function coverageConflict(proposal: CoverageProposal, current: Revision, catalog: Catalog): string | undefined {
  const now = current.requirements.find(r => r.id === proposal.requirementId);
  if (!now) return 'Requirement no longer exists. Request a fresh coverage proposal.';
  try { coveragePlan(proposal.plan, now, catalog, proposal.procedures ?? []); }
  catch (error) { return error instanceof Error ? error.message : 'Evidence is no longer available.'; }
}
/** What changed on the requirement since the proposal's base revision. The owner judges whether the plan still fits. */
export function coverageStale(proposal: CoverageProposal, base: Revision | undefined, current: Revision): string | undefined {
  const before = base?.requirements.find(r => r.id === proposal.requirementId);
  const now = current.requirements.find(r => r.id === proposal.requirementId);
  if (!now) return;
  if (!before) return `Proposed against revision r${proposal.baseRevision}, which no longer holds this requirement. Check that the plan fits before approving.`;
  const planOf = (r: Requirement) => JSON.stringify(r.coverage ? [r.coverage.rationale, r.coverage.criteria] : null);
  const changed = [
    before.statement !== now.statement && 'statement',
    verificationDefinition({ ...before, statement: now.statement }) !== verificationDefinition(now) && 'tests',
    planOf(before) !== planOf(now) && 'coverage plan'
  ].filter((part): part is string => !!part);
  if (!changed.length) return;
  return `The ${new Intl.ListFormat('en').format(changed)} changed after this proposal was made against r${proposal.baseRevision}. Approving replaces the current plan and its test links; check that the plan still fits.`;
}
/** Attaches each requirement's validated draft plan and makes its tests the tests that plan cites. */
export function draftCoverage(requirements: Requirement[], raw: unknown, catalog: Catalog, id: () => string): Requirement[] {
  return requirements.map((requirement, index) => {
    const value = (raw as any)?.[index]?.coverage;
    if (value === undefined || value === null) return { ...requirement, tests: [] };
    const plan = coveragePlan(value, requirement, catalog);
    linkEvidence(requirement, plan, catalog, id);
    return { ...requirement, coverage: plan };
  });
}
