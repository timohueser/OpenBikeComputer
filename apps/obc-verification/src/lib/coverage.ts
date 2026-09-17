import type { CoverageEvidence, CoveragePlan, Requirement } from './types.ts';

export function evidenceTest(requirement: Requirement, evidence: CoverageEvidence) {
  return requirement.tests.find(test => evidence.caseId ? test.kind === 'automated' && test.caseId === evidence.caseId : test.kind === 'manual' && test.id === evidence.testId);
}
export function verificationDefinition(requirement: Requirement): string {
  return JSON.stringify([requirement.statement,
    [...requirement.tests].sort((a, b) => a.id.localeCompare(b.id)).map(t => [t.id, t.kind, t.title, t.caseId, t.steps, t.expected, t.inputs])]);
}
export function coverageIssues(requirement: Requirement, sourceSha?: string): string[] {
  const plan = requirement.coverage;
  if (!plan?.review) return ['Coverage has not been reviewed for this requirement and its test links.'];
  const issues: string[] = [];
  if (sourceSha && plan.sourceSha !== sourceSha) issues.push('Coverage must be reviewed for this source commit.');
  if (plan.conclusion !== 'complete') issues.push('Coverage is partial.');
  if (!plan.criteria.length) issues.push('No acceptance criteria defined.');
  for (const criterion of plan.criteria) {
    if (!criterion.evidence.length || criterion.gap || criterion.evidence.some(e => !evidenceTest(requirement, e))) issues.push(`${criterion.id}: ${criterion.gap || 'Acceptance criterion lacks linked evidence.'}`);
  }
  return issues;
}
export function coverageStatus(requirement: Requirement): string {
  const plan = requirement.coverage;
  if (!plan) return 'Unassessed';
  return plan.conclusion === 'complete' && plan.criteria.length && plan.criteria.every(c => c.evidence.length && !c.gap && c.evidence.every(e => evidenceTest(requirement, e))) ? 'Complete' : 'Partial';
}
export function coverageReviewStatus(requirement: Requirement, sourceSha?: string): string {
  if (!requirement.coverage) return 'Not reviewed';
  return requirement.coverage.review && (!sourceSha || requirement.coverage.sourceSha === sourceSha) ? 'Current' : 'Needs review';
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
