export type Role = 'owner' | 'agent' | 'ci';
export interface Actor { name: string; role: Role; admin?: boolean; provider?: 'local' | 'github'; userId?: string; agentToken?: AgentTokenIdentity }
export interface AgentTokenIdentity { id: string; name: string; issuedBy: Pick<Actor, 'name' | 'provider' | 'userId'> }
export interface AgentToken extends AgentTokenIdentity { createdAt: string; expiresAt: string; lastUsedAt?: string; revokedAt?: string }
export interface ApprovedGitHubUser { id: string; login: string; admin: boolean }
export interface Attachment { id: string; name: string; size: number; sha256: string }
export interface VerificationTest {
  id: string;
  kind: 'automated' | 'manual';
  title: string;
  caseId?: string;
  steps?: string;
  expected?: string;
  inputs: Attachment[];
}
export interface CoverageEvidence { caseId?: string; testId?: string; rationale: string }
export interface AcceptanceCriterion { id: string; statement: string; evidence: CoverageEvidence[]; gap: string }
export interface CoveragePlan { sourceSha: string; conclusion: 'partial' | 'complete'; rationale: string; criteria: AcceptanceCriterion[]; removeTestIds?: string[] }
export interface ReviewedCoverage extends CoveragePlan { review?: { author: string; createdAt: string; proposalId: string } }
export interface Requirement { id: string; title: string; statement: string; group?: string; todo?: boolean; implementationNeeded?: boolean; active: boolean; tests: VerificationTest[]; coverage?: ReviewedCoverage }
export interface Revision { id: number; createdAt: string; author: string; requirements: Requirement[] }
export interface CatalogCase { id: string; suite: string; name: string; file?: string }
export interface Catalog { sourceSha: string; updatedAt: string; cases: CatalogCase[] }
export interface TestResult { caseId: string; status: 'pass' | 'fail' | 'skip' | 'error'; detail?: string }
export interface ManualRun { id: string; requirementId: string; testId: string; result: 'pass' | 'fail' | 'blocked'; device: string; notes: string; evidence: Attachment[]; author: string; createdAt: string }
export interface ReleaseAsset extends Attachment { url?: string }
export interface RequirementException { requirementId: string; reason: string; author: string; createdAt: string }
export interface Candidate {
  id: string; version: string; sourceRef: string; sourceSha: string; revision: Revision;
  createdAt: string; status: 'queued' | 'running' | 'failed' | 'ready' | 'publishing' | 'published';
  ciStatus: 'pending' | 'success' | 'failure'; runId?: number; runAttempt?: number;
  results: TestResult[]; manualRuns: ManualRun[]; assets: ReleaseAsset[];
  releaseUrl?: string; failure?: string; evidenceFrozen?: boolean; exceptions?: RequirementException[];
}
export interface Readiness { ready: boolean; missing: string[]; verified: number; total: number; excepted: number; excluded: number }
export interface Bootstrap { actor: Actor; revision: Revision; catalog: Catalog; candidates: Candidate[]; configured: { github: boolean; oauth: boolean; demo?: boolean } }
export interface LinkProposal { id: string; baseRevision: number; requirementId: string; caseId: string; action: 'add' | 'remove'; reason: string; author: string; agentToken?: AgentTokenIdentity; createdAt: string; status: 'pending' | 'accepted' | 'rejected' | 'superseded'; resolvedByCoverage?: string }
export interface ProposalReview extends LinkProposal { requirement?: Requirement; test?: CatalogCase; conflict?: string }
export interface CoverageProposal { id: string; baseRevision: number; requirementId: string; plan: CoveragePlan; author: string; agentToken?: AgentTokenIdentity; createdAt: string; status: 'pending' | 'accepted' | 'rejected' | 'superseded'; supersedes?: string; feedback?: string; decidedBy?: string; decidedAt?: string }
export interface CoverageProposalReview extends CoverageProposal { requirement?: Requirement; conflict?: string }
