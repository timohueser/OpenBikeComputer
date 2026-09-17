import type { AcceptanceCriterion, Catalog, CoverageEvidence, CoveragePlan, Requirement } from './types.ts';

export function evidenceTest(requirement: Requirement, evidence: CoverageEvidence) {
  return requirement.tests.find(test => evidence.caseId ? test.kind === 'automated' && test.caseId === evidence.caseId : test.kind === 'manual' && test.id === evidence.testId);
}
export function evidenceKey(evidence: CoverageEvidence): string { return evidence.caseId ?? evidence.testId ?? ''; }
/** A criterion counts as covered when every piece of evidence is a linked test and no gap is recorded. */
export function criterionCovered(requirement: Requirement, criterion: AcceptanceCriterion): boolean {
  return criterion.evidence.length > 0 && !criterion.gap.trim() && criterion.evidence.every(e => evidenceTest(requirement, e));
}
/** A proposed plan counts as covered on its own terms: its evidence is linked when it is approved. */
export function planCovered(plan: CoveragePlan): boolean {
  return plan.criteria.length > 0 && plan.criteria.every(c => c.evidence.length > 0 && !c.gap.trim());
}
export function verificationDefinition(requirement: Requirement): string {
  return JSON.stringify([requirement.statement,
    [...requirement.tests].sort((a, b) => a.id.localeCompare(b.id)).map(t => [t.id, t.kind, t.title, t.caseId, t.steps, t.expected, t.inputs])]);
}
/** Everything an approval attests to. A change here clears the approval. */
export function coverageDefinition(requirement: Requirement): string {
  const plan = requirement.coverage;
  return JSON.stringify([verificationDefinition(requirement), plan ? [plan.rationale, plan.criteria] : null]);
}
/** The first reason a plan cannot be saved, if any. Shared by the editor and the server. */
export function planProblem(plan: CoveragePlan): string | undefined {
  if (!plan.criteria.length) return 'Add at least one criterion.';
  if (plan.criteria.some(c => !c.statement.trim())) return 'Every criterion needs a statement.';
  if (plan.criteria.some(c => c.evidence.some(e => !e.rationale.trim()))) return 'Say what each piece of evidence proves.';
}
/** A pending link suggestion that a proposed plan already contains is resolved when that plan is approved. */
export function absorbedBy(plan: CoveragePlan, requirement: Requirement, suggestion: { caseId: string; action: 'add' | 'remove' }): boolean {
  return suggestion.action === 'add'
    ? plan.criteria.some(c => c.evidence.some(e => e.caseId === suggestion.caseId))
    : (plan.removeTestIds ?? []).some(id => requirement.tests.find(t => t.id === id)?.caseId === suggestion.caseId);
}
export function planBlank(plan: CoveragePlan): boolean {
  return !plan.rationale.trim() && plan.criteria.every(c => !c.statement.trim() && !c.evidence.length && !c.gap.trim());
}
export function coverageIssues(requirement: Requirement, sourceSha?: string): string[] {
  const plan = requirement.coverage;
  if (!plan?.review) return ['Coverage has not been approved for this requirement and its test links.'];
  const issues: string[] = [];
  if (sourceSha && plan.review.sourceSha !== sourceSha) issues.push('Coverage must be approved for this source commit.');
  if (!plan.criteria.length) issues.push('No acceptance criteria defined.');
  for (const criterion of plan.criteria) {
    if (!criterionCovered(requirement, criterion)) issues.push(`“${criterion.statement}”: ${criterion.gap || (criterion.evidence.length ? 'evidence is no longer a linked test.' : 'no evidence mapped.')}`);
  }
  return issues;
}

export type CoverageState = 'unassessed' | 'needs-review' | 'partial' | 'covered';
export interface CoverageSummary { state: CoverageState; label: string; covered: number; total: number }
const labels: Record<CoverageState, string> = { unassessed: 'Not assessed', 'needs-review': 'Needs review', partial: 'Partial', covered: 'Covered' };
/** One state per requirement. "Covered" is the only state that satisfies the release gate. */
export function coverageSummary(requirement: Requirement, sourceSha?: string): CoverageSummary {
  const plan = requirement.coverage;
  const total = plan?.criteria.length ?? 0;
  const covered = plan?.criteria.filter(c => criterionCovered(requirement, c)).length ?? 0;
  const state: CoverageState = !plan ? 'unassessed'
    : !plan.review || (sourceSha && plan.review.sourceSha !== sourceSha) ? 'needs-review'
    : total > 0 && covered === total ? 'covered' : 'partial';
  return { state, label: labels[state], covered, total };
}
export function mappedBy(plan: CoveragePlan, test: Requirement['tests'][number]): boolean {
  return plan.criteria.some(c => c.evidence.some(e => test.kind === 'automated' ? e.caseId === test.caseId : e.testId === test.id));
}
export function coverageChanges(requirement: Requirement, plan: CoveragePlan) {
  const before = requirement.coverage?.criteria ?? [];
  return {
    added: plan.criteria.filter(c => !before.some(old => old.id === c.id)),
    changed: plan.criteria.filter(c => before.some(old => old.id === c.id && JSON.stringify(old) !== JSON.stringify(c))),
    removed: before.filter(c => !plan.criteria.some(next => next.id === c.id)),
    addedCases: [...new Set(plan.criteria.flatMap(c => c.evidence.flatMap(e => e.caseId && !requirement.tests.some(t => t.caseId === e.caseId) ? [e.caseId] : [])))],
    removedTests: requirement.tests.filter(t => plan.removeTestIds?.includes(t.id)),
    unmappedTests: requirement.tests.filter(t => !plan.removeTestIds?.includes(t.id) && !mappedBy(plan, t))
  };
}
/** Sentences describing the test links that approving this plan adds or removes. */
export function linkChanges(requirement: Requirement, plan: CoveragePlan, catalog: Catalog): string[] {
  const delta = coverageChanges(requirement, plan);
  const name = (id: string) => catalog.cases.find(c => c.id === id)?.name ?? id;
  const list = (items: string[]) => `${items.length} ${items.length === 1 ? 'test' : 'tests'}: ${items.join(', ')}.`;
  return [
    ...(delta.addedCases.length ? [`Links ${list(delta.addedCases.map(name))}`] : []),
    ...(delta.removedTests.length ? [`Unlinks ${list(delta.removedTests.map(t => t.kind === 'manual' ? `${t.title} (manual procedure)` : t.title))}`] : [])
  ];
}
