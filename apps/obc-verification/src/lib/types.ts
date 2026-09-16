export type Role = 'owner' | 'agent' | 'ci';
export interface Actor { name: string; role: Role; admin?: boolean; provider?: 'local' | 'github'; userId?: string }
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
export interface Requirement { id: string; title: string; statement: string; group?: string; todo?: boolean; implementationNeeded?: boolean; active: boolean; tests: VerificationTest[] }
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
export interface Bootstrap { actor: Actor; revision: Revision; catalog: Catalog; candidates: Candidate[]; configured: { github: boolean; oauth: boolean } }
export interface LinkProposal { id: string; baseRevision: number; requirementId: string; caseId: string; action: 'add' | 'remove'; reason: string; author: string; createdAt: string; status: 'pending' | 'accepted' | 'rejected' }
