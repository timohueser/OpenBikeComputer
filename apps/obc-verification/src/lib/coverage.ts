import type { AcceptanceCriterion, CoverageEvidence, CoveragePlan, Requirement, VerificationTest } from './types.ts';

export function evidenceTest(requirement: Requirement, evidence: CoverageEvidence) {
  return requirement.tests.find(test => evidence.caseId ? test.kind === 'automated' && test.caseId === evidence.caseId : test.kind === 'manual' && test.id === evidence.testId);
}
/** The verification of a requirement: every test its plan cites, once, in plan order. */
export function citedTests(requirement: Requirement): VerificationTest[] {
  const seen = new Set<string>();
  return (requirement.coverage?.criteria ?? []).flatMap(c => c.evidence.flatMap(e => {
    const test = evidenceTest(requirement, e);
    if (!test || seen.has(test.id)) return [];
    seen.add(test.id);
    return [test];
  }));
}
export function evidenceKey(evidence: CoverageEvidence): string { return evidence.caseId ?? evidence.testId ?? ''; }
/** A criterion counts as covered when every piece of evidence is a linked test and no gap is recorded. */
export function criterionCovered(requirement: Requirement, criterion: AcceptanceCriterion): boolean {
  return criterion.evidence.length > 0 && !criterion.gap.trim() && criterion.evidence.every(e => evidenceTest(requirement, e));
}
/** Criteria a proposed plan covers on its own terms: its evidence is linked when it is approved. */
export function planCoveredCount(plan: CoveragePlan): number {
  return plan.criteria.filter(c => c.evidence.length > 0 && !c.gap.trim()).length;
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
export function planBlank(plan: CoveragePlan): boolean {
  return !plan.rationale.trim() && plan.criteria.every(c => !c.statement.trim() && !c.evidence.length && !c.gap.trim());
}
export function coverageIssues(requirement: Requirement): string[] {
  const plan = requirement.coverage;
  if (!plan?.review) return ['Coverage has not been approved for this requirement and its test links.'];
  const issues: string[] = [];
  if (!plan.criteria.length) issues.push('No acceptance criteria defined.');
  for (const criterion of plan.criteria) {
    if (!criterionCovered(requirement, criterion)) issues.push(`“${criterion.statement}”: ${criterion.gap || (criterion.evidence.length ? 'evidence is not available.' : 'no evidence mapped.')}`);
  }
  return issues;
}

export type CoverageState = 'unassessed' | 'needs-review' | 'partial' | 'covered';
export interface CoverageSummary { state: CoverageState; label: string; covered: number; total: number }
const labels: Record<CoverageState, string> = { unassessed: 'Not assessed', 'needs-review': 'Needs review', partial: 'Partial', covered: 'Covered' };
/** One state per requirement. "Covered" is the only state that satisfies the release gate. */
export function coverageSummary(requirement: Requirement): CoverageSummary {
  const plan = requirement.coverage;
  const total = plan?.criteria.length ?? 0;
  const covered = plan?.criteria.filter(c => criterionCovered(requirement, c)).length ?? 0;
  const state: CoverageState = !plan ? 'unassessed'
    : !plan.review ? 'needs-review'
    : total > 0 && covered === total ? 'covered' : 'partial';
  return { state, label: labels[state], covered, total };
}
export function coverageChanges(requirement: Requirement, plan: CoveragePlan) {
  const before = requirement.coverage?.criteria ?? [];
  return {
    added: plan.criteria.filter(c => !before.some(old => old.id === c.id)),
    changed: plan.criteria.filter(c => before.some(old => old.id === c.id && JSON.stringify(old) !== JSON.stringify(c))),
    removed: before.filter(c => !plan.criteria.some(next => next.id === c.id)),
    /** Manual procedures this plan cites nowhere. Saving the plan deletes them with their content. */
    deletedProcedures: requirement.tests.filter(t => t.kind === 'manual' && !plan.criteria.some(c => c.evidence.some(e => e.testId === t.id)))
  };
}
