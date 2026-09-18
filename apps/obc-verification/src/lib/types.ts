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
  /** Manual tests only: `human` for a test a person must always run, `until-automated` for one that
   *  waits for an automated test. Absent when it is not set. */
  manualReason?: ManualReason;
}
/** One cited test, and what the plan says it proves. `level` is the scope of that test. It is
 *  optional, because plans written before it existed do not have one. */
export interface CoverageEvidence { caseId?: string; testId?: string; rationale: string; level?: TestLevel }
/** How much of the product a test exercises: one module, several together, or the assembled product.
 *  It says nothing about who runs the test — automated evidence cites a catalogue case, and manual
 *  evidence cites a procedure, so the plan already carries that. */
export type TestLevel = 'unit' | 'integration' | 'system';
export const TEST_LEVELS: TestLevel[] = ['unit', 'integration', 'system'];
/** The type of a manual test. Automated tests have none. */
export type ManualReason = 'human' | 'until-automated';
export const MANUAL_REASONS: ManualReason[] = ['human', 'until-automated'];
/** One short description per kind, for the panel that any pill opens. */
export const TEST_KIND_NOTES: { key: string; title: string; note: string }[] = [
  { key: 'unit', title: 'Unit', note: 'One module, on a CI runner. It shows that the module is correct on its own.' },
  { key: 'integration', title: 'Integration', note: 'Two or more modules together, on a CI runner or a hardware rig. It shows that the modules work correctly together.' },
  { key: 'system', title: 'System', note: 'The complete product, on real or simulated hardware. It shows that the product does what the requirement states.' },
  { key: 'human', title: 'Human check', note: 'A person must run this test. For example, a real phone with a real device, or a check that needs human judgement. A release runs these tests by hand.' },
  { key: 'until-automated', title: 'Until automated', note: 'A person runs this test because no automated test exists yet. Replace it when one is written.' },
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
