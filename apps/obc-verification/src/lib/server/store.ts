import { DatabaseSync } from 'node:sqlite';
import { mkdirSync } from 'node:fs';
import { resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
import type { ApprovedGitHubUser, Attachment, Candidate, Catalog, LinkProposal, Requirement, Revision } from '../types.ts';
import { assert, Problem, refresh } from './domain.ts';

const referencedRevisions = `SELECT json_extract(body, '$.revision.id') FROM records
  WHERE kind IN ('candidate', 'publication') AND json_type(body, '$.revision.id') = 'integer'`;

export class Store {
  db: DatabaseSync;
  directory: string;
  constructor(directory: string) {
    this.directory = directory;
    mkdirSync(directory, { recursive: true, mode: 0o700 });
    mkdirSync(resolve(directory, 'files'), { recursive: true, mode: 0o700 });
    this.db = new DatabaseSync(resolve(directory, 'verification.sqlite'));
    this.db.exec(`PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;
      CREATE TABLE IF NOT EXISTS revisions (id INTEGER PRIMARY KEY AUTOINCREMENT, created_at TEXT NOT NULL, author TEXT NOT NULL, body TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS records (seq INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, id TEXT NOT NULL, body TEXT NOT NULL);
      CREATE INDEX IF NOT EXISTS record_lookup ON records(kind,id,seq);
      CREATE TABLE IF NOT EXISTS sessions (hash TEXT PRIMARY KEY, actor TEXT NOT NULL, expires INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS github_users (id TEXT PRIMARY KEY, login TEXT NOT NULL, admin INTEGER NOT NULL CHECK(admin IN (0,1)));
      CREATE TABLE IF NOT EXISTS local_admin (id INTEGER PRIMARY KEY CHECK(id=1), password_hash TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS login_attempts (key TEXT PRIMARY KEY, count INTEGER NOT NULL, expires INTEGER NOT NULL);`);
    const initialHash = process.env.VERIFICATION_OWNER_PASSWORD_HASH || '';
    if (/^[a-f0-9]{32}:[a-f0-9]{128}$/.test(initialHash)) this.db.prepare('INSERT OR IGNORE INTO local_admin(id,password_hash) VALUES(1,?)').run(initialHash);
    if (!this.db.prepare('SELECT id FROM revisions LIMIT 1').get()) this.saveRevision(0, 'Example', [{
      id: 'EXAMPLE-001', title: 'Example — large route upload', active: false,
      statement: '**Illustrative only.** Upload a GPX route containing up to 500,000 GPS points. Replace this example with a requirement you have reviewed before activating it.', tests: []
    }]);
    this.atomic(() => this.rememberRequirementNumbers([
      ...this.revisions().flatMap(r => r.requirements),
      ...['candidate', 'publication'].flatMap(kind => this.list<Candidate>(kind).flatMap(c => c.revision.requirements))
    ]));
  }
  atomic<T>(operation: () => T): T {
    this.db.exec('BEGIN IMMEDIATE');
    try { const result = operation(); this.db.exec('COMMIT'); return result; }
    catch (error) { this.db.exec('ROLLBACK'); throw error; }
  }
  latestRevision(): Revision { return this.revisions()[0]; }
  revisions(): Revision[] {
    return this.db.prepare('SELECT * FROM revisions ORDER BY id DESC').all().map((row) => ({ id: Number(row.id), author: String(row.author), createdAt: String(row.created_at), requirements: JSON.parse(String(row.body)) }));
  }
  revision(id: number): Revision {
    const result = this.revisions().find((r) => r.id === id);
    if (!result) throw new Problem(404, 'Revision not found.');
    return result;
  }
  private rememberRequirementNumbers(requirements: Requirement[]): void {
    const previous = this.maybe<number>('sequence', 'requirement') ?? 0;
    const highest = requirements.reduce((highest, requirement) => {
      const match = /^SYS-(\d+)$/.exec(requirement.id);
      const number = match ? Number(match[1]) : 0;
      return Number.isSafeInteger(number) ? Math.max(highest, number) : highest;
    }, previous);
    if (highest > previous) this.put('sequence', 'requirement', highest);
  }
  reserveRequirementIds(count = 1): string[] {
    return this.atomic(() => {
      const first = (this.maybe<number>('sequence', 'requirement') ?? 0) + 1;
      const last = first + count - 1;
      assert(Number.isSafeInteger(last), 'Requirement number limit reached.');
      this.put('sequence', 'requirement', last);
      return Array.from({ length: count }, (_, i) => `SYS-${String(first + i).padStart(3, '0')}`);
    });
  }
  reserveRequirementId(): string { return this.reserveRequirementIds(1)[0]; }
  saveRevision(base: number, author: string, requirements: Requirement[]): Revision {
    return this.atomic(() => {
      const row = this.db.prepare('SELECT MAX(id) AS id FROM revisions').get();
      assert(Number(row?.id ?? 0) === base, 'Requirements changed. Reload before saving.', 409);
      this.rememberRequirementNumbers(requirements);
      const createdAt = new Date().toISOString();
      const inserted = this.db.prepare('INSERT INTO revisions(created_at,author,body) VALUES (?,?,?)').run(createdAt, author, JSON.stringify(requirements));
      return { id: Number(inserted.lastInsertRowid), author, createdAt, requirements };
    });
  }
  historySummary() {
    const revisions = this.revisions();
    const current = revisions[0];
    const protectedCount = this.db.prepare(`SELECT COUNT(*) AS count FROM revisions WHERE id IN (${referencedRevisions})`).get();
    return { baseRevision: current.id, revisionCount: revisions.length, protectedRevisionCount: Number(protectedCount?.count ?? 0),
      requirementCount: current.requirements.length, testCount: current.requirements.reduce((count, r) => count + r.tests.length, 0) };
  }
  clearHistory(base: number, author: string, clearCurrent: boolean): Revision {
    return this.atomic(() => {
      const current = this.latestRevision();
      assert(current.id === base, 'Requirements changed. Refresh the preview and confirm again.', 409);
      const createdAt = new Date().toISOString();
      const requirements = clearCurrent ? [] : current.requirements;
      const inserted = this.db.prepare('INSERT INTO revisions(created_at,author,body) VALUES(?,?,?)').run(createdAt, author, JSON.stringify(requirements));
      const id = Number(inserted.lastInsertRowid);
      this.db.prepare(`DELETE FROM revisions WHERE id <> ? AND id NOT IN (${referencedRevisions})`).run(id);
      this.db.prepare("DELETE FROM records WHERE kind='proposal'").run();
      return { id, author, createdAt, requirements };
    });
  }
  put(kind: string, id: string, value: unknown): void {
    this.db.prepare('INSERT INTO records(kind,id,body) VALUES (?,?,?)').run(kind, id, JSON.stringify(value));
  }
  get<T>(kind: string, id: string): T {
    const row = this.db.prepare('SELECT body FROM records WHERE kind=? AND id=? ORDER BY seq DESC LIMIT 1').get(kind, id);
    if (!row) throw new Problem(404, `${kind} not found.`);
    return JSON.parse(String(row.body)) as T;
  }
  maybe<T>(kind: string, id: string): T | undefined { try { return this.get<T>(kind, id); } catch (error) { if (error instanceof Problem && error.status === 404) return; throw error; } }
  list<T>(kind: string): T[] {
    return this.db.prepare('SELECT body FROM records WHERE seq IN (SELECT MAX(seq) FROM records WHERE kind=? GROUP BY id) ORDER BY seq DESC').all(kind).map((r) => JSON.parse(String(r.body)) as T);
  }
  candidate(id: string): Candidate { return this.get<Candidate>('candidate', id); }
  updateCandidate(id: string, change: (candidate: Candidate) => void): Candidate {
    return this.atomic(() => { const candidate = this.candidate(id); change(candidate); refresh(candidate); this.put('candidate', id, candidate); return candidate; });
  }
  file(id: string): Attachment { return this.get<Attachment>('file', id); }
  catalog(): Catalog { return this.maybe<Catalog>('catalog', 'current') ?? { sourceSha: '', updatedAt: '', cases: [] }; }
  proposals(): LinkProposal[] { return this.list<LinkProposal>('proposal'); }
  githubUsers(): ApprovedGitHubUser[] {
    return this.db.prepare('SELECT id,login,admin FROM github_users ORDER BY login COLLATE NOCASE').all().map((row) => ({ id: String(row.id), login: String(row.login), admin: row.admin === 1 }));
  }
  githubUser(id: string): ApprovedGitHubUser | undefined {
    const row = this.db.prepare('SELECT id,login,admin FROM github_users WHERE id=?').get(id);
    return row ? { id: String(row.id), login: String(row.login), admin: row.admin === 1 } : undefined;
  }
  localPasswordHash(): string { return String(this.db.prepare('SELECT password_hash FROM local_admin WHERE id=1').get()?.password_hash ?? ''); }
  id(): string { return randomUUID(); }
}
let instance: Store | undefined;
export function store(): Store { return instance ??= new Store(resolve(process.env.VERIFICATION_DATA_DIR || './data')); }
