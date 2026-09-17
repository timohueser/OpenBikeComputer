import { isDeepStrictEqual } from 'node:util';
import type { AcceptanceCriterion, Catalog, CoveragePlan, CoverageProposal, Requirement, Revision } from '../types.ts';
import { mappedBy, planProblem, verificationDefinition } from '../coverage.ts';
import { assert, identifier, text } from './domain.ts';

/** Validates a plan for a requirement. Automated evidence may name any catalogue case; it is linked when the plan is saved. */
export function coveragePlan(value: unknown, requirement: Requirement, catalog: Catalog): CoveragePlan {
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
      assert(ref.caseId ? caseIds.has(ref.caseId) || requirement.tests.some(t => t.caseId === ref.caseId) : requirement.tests.some(t => t.id === ref.testId && t.kind === 'manual'), 'Evidence test is not available.');
      const key = JSON.stringify(ref);
      assert(!seen.has(key), 'Duplicate evidence for a criterion.'); seen.add(key);
      return { ...ref, rationale: text(e.rationale, 'Evidence rationale', 5000) };
    });
    return { id, statement: text(entry.statement, 'Acceptance criterion', 5000), evidence, gap: entry.gap.trim() };
  });
  const result: CoveragePlan = { rationale: plan.rationale.trim(), criteria };
  const problem = planProblem(result);
  assert(!problem, problem ?? '');
  const addedCases = new Set(criteria.flatMap(c => c.evidence.flatMap(e => e.caseId && !requirement.tests.some(t => t.caseId === e.caseId) ? [e.caseId] : [])));
  const removals = plan.removeTestIds ?? [];
  assert(Array.isArray(removals) && removals.length <= 100, 'At most 100 test links can be removed.');
  const removeTestIds: string[] = removals.map((id: unknown) => identifier(id, 'Removed test ID'));
  assert(new Set(removeTestIds).size === removeTestIds.length, 'Duplicate test removal.');
  for (const id of removeTestIds) {
    const test = requirement.tests.find(t => t.id === id);
    assert(test, 'The test to remove is no longer linked.');
    assert(!mappedBy(result, test), 'Cannot remove a test used by the proposed coverage.');
  }
  assert(requirement.tests.length - removeTestIds.length + addedCases.size <= 100, 'At most 100 tests can be linked to a requirement.');
  return { ...result, ...(removeTestIds.length ? { removeTestIds } : {}) };
}
export function commitSha(value: unknown): string {
  assert(typeof value === 'string' && /^[a-f0-9]{40}$/.test(value), 'Use the exact 40-character source commit that you assessed.');
  return value;
}
/** Links every catalogue case a plan cites that the requirement does not carry yet. */
export function linkEvidence(requirement: Requirement, plan: CoveragePlan, catalog: Catalog, id: () => string): void {
  for (const evidence of plan.criteria.flatMap(c => c.evidence)) {
    if (evidence.caseId && !requirement.tests.some(t => t.caseId === evidence.caseId)) {
      const found = catalog.cases.find(c => c.id === evidence.caseId)!;
      requirement.tests.push({ id: id(), title: found.name.slice(0, 300), kind: 'automated', caseId: found.id, inputs: [] });
    }
  }
}

export function coverageConflict(proposal: CoverageProposal, base: Revision | undefined, current: Revision, catalog: Catalog): string | undefined {
  const before = base?.requirements.find(r => r.id === proposal.requirementId);
  const now = current.requirements.find(r => r.id === proposal.requirementId);
  if (!before || !now) return 'Requirement no longer exists. Request a fresh coverage proposal.';
  if (verificationDefinition(before) !== verificationDefinition(now) || !isDeepStrictEqual(before.coverage, now.coverage)) return 'Requirement, test links, or coverage plan changed. Request an updated proposal.';
  try { coveragePlan(proposal.plan, now, catalog); }
  catch (error) { return error instanceof Error ? error.message : 'Evidence is no longer available.'; }
}
/** Attaches each requirement's validated draft plan and links the catalogue cases it cites. */
export function draftCoverage(requirements: Requirement[], raw: unknown, catalog: Catalog, id: () => string): Requirement[] {
  return requirements.map((requirement, index) => {
    const value = (raw as any)?.[index]?.coverage;
    if (value === undefined || value === null) return requirement;
    const { removeTestIds: ignored, ...plan } = coveragePlan(value, requirement, catalog);
    linkEvidence(requirement, plan, catalog, id);
    return { ...requirement, coverage: plan };
  });
}
