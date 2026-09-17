import { isDeepStrictEqual } from 'node:util';
import type { AcceptanceCriterion, Catalog, CoveragePlan, CoverageProposal, Requirement, Revision } from '../types.ts';
import { verificationDefinition } from '../coverage.ts';
import { assert, identifier, text } from './domain.ts';

export function coveragePlan(value: unknown, requirement: Requirement, catalog: Catalog): CoveragePlan {
  assert(value && typeof value === 'object', 'Coverage plan is required.');
  const plan = value as Record<string, any>;
  assert(typeof plan.sourceSha === 'string' && /^[a-f0-9]{40}$/.test(plan.sourceSha), 'Use the exact source commit reviewed by the agent.');
  assert(plan.conclusion === 'partial' || plan.conclusion === 'complete', 'Choose partial or complete coverage.');
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
      assert(ref.caseId ? caseIds.has(ref.caseId) : requirement.tests.some(t => t.id === ref.testId && t.kind === 'manual'), 'Evidence test is not available.');
      const key = JSON.stringify(ref);
      assert(!seen.has(key), 'Duplicate evidence for a criterion.'); seen.add(key);
      return { ...ref, rationale: text(e.rationale, 'Evidence rationale', 5000) };
    });
    const gap = entry.gap.trim();
    assert(plan.conclusion !== 'complete' || (evidence.length > 0 && !gap), 'Complete coverage requires evidence and no remaining gap for every criterion.');
    return { id, statement: text(entry.statement, 'Acceptance criterion', 5000), evidence, gap };
  });
  const addedCases = new Set(criteria.flatMap(c => c.evidence.flatMap(e => e.caseId && !requirement.tests.some(t => t.caseId === e.caseId) ? [e.caseId] : [])));
  assert(requirement.tests.length + addedCases.size <= 100, 'At most 100 tests can be linked to a requirement.');
  return { sourceSha: plan.sourceSha, conclusion: plan.conclusion, rationale: text(plan.rationale, 'Coverage rationale', 10000), criteria };
}

export function coverageConflict(proposal: CoverageProposal, base: Revision | undefined, current: Revision, catalog: Catalog): string | undefined {
  const before = base?.requirements.find(r => r.id === proposal.requirementId);
  const now = current.requirements.find(r => r.id === proposal.requirementId);
  if (!before || !now) return 'Requirement no longer exists. Request a fresh coverage proposal.';
  if (verificationDefinition(before) !== verificationDefinition(now) || !isDeepStrictEqual(before.coverage, now.coverage)) return 'Requirement, test links, or coverage plan changed. Request an updated proposal.';
  try { coveragePlan(proposal.plan, now, catalog); }
  catch (error) { return error instanceof Error ? error.message : 'Evidence is no longer available.'; }
}
