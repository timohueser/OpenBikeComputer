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
  /** Manual tests only: whether a person is the permanent answer, or holds the place for a test
   *  that should exist. Absent means it has not been said. */
  manualReason?: ManualReason;
}
/** One cited test, and what the plan says it proves. `level` is the auditor's judgement of the
 *  test's scope; it is optional because plans written before it existed do not carry it. */
export interface CoverageEvidence { caseId?: string; testId?: string; rationale: string; level?: TestLevel }
/** How much of the product a test exercises: one module, several together, or the assembled product.
 *  It says nothing about who runs the test — automated evidence cites a catalogue case, and manual
 *  evidence cites a procedure, so the plan already carries that. */
export type TestLevel = 'unit' | 'integration' | 'system';
export const TEST_LEVELS: TestLevel[] = ['unit', 'integration', 'system'];
/** Why a procedure is a person's job. Automated tests have none. */
export type ManualReason = 'human' | 'until-automated';
export const MANUAL_REASONS: ManualReason[] = ['human', 'until-automated'];
/** One short sentence per kind, for the explainer any pill opens. Level and manual are different
 *  questions: the level says how much of the product runs, and manual says who runs it. */
export const TEST_KIND_NOTES: { key: string; title: string; note: string }[] = [
  { key: 'unit', title: 'Unit', note: 'One module on its own, on a CI runner. It shows that a part behaves correctly by itself.' },
  { key: 'integration', title: 'Integration', note: 'Several modules together, on a CI runner or on a hardware rig. It shows that the parts agree with each other.' },
  { key: 'system', title: 'System', note: 'The assembled product, on real or simulated hardware. It shows that the whole thing does what a rider asks of it.' },
  { key: 'human', title: 'Manual · human', note: 'A person is the permanent answer: a real phone against a real device, or a judgement no rig can make. These are the checks a release runs by hand.' },
  { key: 'until-automated', title: 'Manual · until automated', note: 'A person runs it because the automated or rig test does not exist yet. It holds the place for one that should.' },
];
/** The test to build for a gap: one level, one sentence. A criterion with no gap has nothing to build. */
export interface ProposedTest { level: TestLevel; summary: string }
export interface AcceptanceCriterion { id: string; statement: string; evidence: CoverageEvidence[]; gap: string; next?: ProposedTest }
export interface CoveragePlan { rationale: string; criteria: AcceptanceCriterion[] }
/** An owner's approval of the saved statement, tests, and plan. It records the catalogue commit of that moment. */
export interface CoverageReview { author: string; createdAt: string; sourceSha?: string; proposalId?: string }
export interface ReviewedCoverage extends CoveragePlan { review?: CoverageReview }
/** `tests` are exactly the tests the coverage plan cites. Results and manual runs refer to them by ID. */
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
/** `procedures` are new manual tests the plan cites; approval creates them on the requirement. */
export interface CoverageProposal { id: string; baseRevision: number; requirementId: string; sourceSha: string; plan: CoveragePlan; procedures?: VerificationTest[]; author: string; agentToken?: AgentTokenIdentity; createdAt: string; status: 'pending' | 'accepted' | 'rejected' | 'superseded'; supersedes?: string; feedback?: string; decidedBy?: string; decidedAt?: string }
/** `conflict` blocks approval; `stale` names what changed since the base revision and leaves the decision to the owner. */
export interface CoverageProposalReview extends CoverageProposal { requirement?: Requirement; conflict?: string; stale?: string }
