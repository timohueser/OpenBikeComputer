import type { CoverageEvidence, Requirement } from './types.ts';

export function evidenceTest(requirement: Requirement, evidence: CoverageEvidence) {
  return requirement.tests.find(test => evidence.caseId ? test.kind === 'automated' && test.caseId === evidence.caseId : test.kind === 'manual' && test.id === evidence.testId);
}
export function verificationDefinition(requirement: Requirement): string {
  return JSON.stringify([requirement.title, requirement.statement, requirement.group ?? '', requirement.active, !!requirement.todo, !!requirement.implementationNeeded,
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
export function coverageStatus(requirement: Requirement, sourceSha?: string): string {
  if (!requirement.coverage) return 'Not reviewed';
  if (!requirement.coverage.review || (sourceSha && requirement.coverage.sourceSha !== sourceSha)) return 'Needs review';
  return coverageIssues(requirement, sourceSha).length ? 'Partial' : 'Reviewed as complete';
}
