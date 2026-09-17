import { isDeepStrictEqual } from 'node:util';
import type { Catalog, LinkProposal, Requirement, Revision } from '../types.ts';

export function proposalConflict(proposal: LinkProposal, base: Revision, current: Revision, catalog: Catalog): string | undefined {
  const before = base.requirements.find(r => r.id === proposal.requirementId);
  const now = current.requirements.find(r => r.id === proposal.requirementId);
  if (!before || !now) return 'Requirement no longer exists. Reject this proposal.';
  const definition = (r: Requirement) => [r.title, r.statement, r.group ?? '', r.active, !!r.todo, !!r.implementationNeeded];
  if (!isDeepStrictEqual(definition(before), definition(now))) return 'Requirement changed. Request a fresh proposal.';
  const link = (r: Requirement) => r.tests.find(t => t.kind === 'automated' && t.caseId === proposal.caseId);
  if (proposal.action === 'add') {
    if (link(now)) return 'Test is already linked.';
    if (link(before)) return 'This link was removed after the proposal. Request a fresh proposal.';
    if (!catalog.cases.some(c => c.id === proposal.caseId)) return 'Test is no longer in the catalogue.';
    if (now.tests.length >= 100) return 'At most 100 tests can be linked to a requirement.';
  } else {
    if (!link(now)) return 'Test is no longer linked.';
    if (!link(before) || !isDeepStrictEqual(link(before), link(now))) return 'Test link changed. Request a fresh proposal.';
  }
}
