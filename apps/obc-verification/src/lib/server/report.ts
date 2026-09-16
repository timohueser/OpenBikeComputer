import { marked } from 'marked';
import sanitize from 'sanitize-html';
import type { Candidate } from '../types.ts';
import { readiness, requirementIssues } from './domain.ts';
const escape = (s: unknown) => String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!);
const markdown = (source: string) => sanitize(marked.parse(source, { async: false }), { allowedTags: sanitize.defaults.allowedTags, allowedAttributes: { a: ['href', 'title'] }, allowedSchemes: ['https', 'http'] });
export function report(candidate: Candidate): string {
  const state = readiness(candidate);
  const exceptions = candidate.exceptions ?? [];
  const excluded = candidate.revision.requirements.filter(r => !r.active);
  const results = new Map(candidate.results.map(result => [result.caseId, result.status]));
  const limitations = [state.excepted ? 'exceptions' : '', excluded.length ? 'exclusions' : ''].filter(Boolean).join(' and ');
  const outcome = !state.ready ? 'Incomplete' : limitations ? `Accepted with ${limitations}` : 'Verified';
  return `<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>OBC ${escape(candidate.version)} verification</title>
<style>body{font:16px/1.6 system-ui;max-width:900px;margin:40px auto;padding:24px;color:#172b2b}article{border-top:1px solid #ccd;padding:24px 0}pre{white-space:pre-wrap}small{color:#567}code{overflow-wrap:anywhere}.exceptions{border:2px solid #b36a00;background:#fff8e8;padding:20px;margin:24px 0}.reason{white-space:pre-wrap}.todo{color:#9b3b12;font-weight:600}</style>
<h1>OpenBikeComputer ${escape(candidate.version)}</h1><p>Candidate ${escape(candidate.id)} · ${escape(candidate.createdAt)}</p>
<p>Source <code>${escape(candidate.sourceSha)}</code> · Requirements revision ${candidate.revision.id}</p>
<h2>${outcome}</h2><p>${state.verified}/${state.total} requirements verified · ${state.excepted} accepted with exception · ${excluded.length} excluded</p>
${excluded.length ? `<section class="exceptions"><h2>Excluded requirements</h2><p>These requirements are excluded by this requirements revision. They are not required for this candidate and are not counted as verified.</p>${excluded.map(r => `<h3>${escape(r.id)} — ${escape(r.title)}</h3>${r.todo ? '<p class="todo">Definition incomplete</p>' : ''}${r.implementationNeeded ? '<p class="todo">Implementation needed</p>' : ''}`).join('')}</section>` : ''}
${exceptions.length ? `<section class="exceptions"><h2>Accepted requirement exceptions</h2><p>These requirements are not counted as verified. Original test outcomes are retained below. Each decision applies only to this candidate.</p>${exceptions.map(e => `<h3>${escape(e.requirementId)} — ${escape(candidate.revision.requirements.find(r => r.id === e.requirementId)?.title || '')}</h3><p class="reason">${escape(e.reason)}</p><p><small>Accepted by ${escape(e.author)} · ${escape(e.createdAt)}</small></p>`).join('')}</section>` : ''}
${state.missing.map(m => `<p>${escape(m)}</p>`).join('')}
${candidate.revision.requirements.map(r => {
    const exception = exceptions.find(e => e.requirementId === r.id);
    const issues = r.active ? requirementIssues(candidate, r, results) : [];
    return `<article><h2>${escape(r.id)} — ${escape(r.title)}${r.active ? '' : ' (excluded from release)'}</h2>
${r.group ? `<p><small>Group: ${escape(r.group)}</small></p>` : ''}
${r.todo ? '<p class="todo">Definition incomplete — requirement definition needs work.</p>' : ''}
${r.implementationNeeded ? '<p class="todo">Implementation needed — known implementation gap.</p>' : ''}
${exception ? '<p><strong>Accepted with exception — not verified.</strong> See the decision above.</p>' : ''}
${markdown(r.statement)}${issues.map(issue => `<p>${escape(issue)}</p>`).join('')}
${r.tests.map(t => `<h3>${escape(t.title)} · ${t.kind}</h3>${t.kind === 'manual' ? markdown(t.steps || '') + '<h4>Expected</h4>' + markdown(t.expected || '') : `<p><code>${escape(t.caseId)}</code>: ${escape(results.get(t.caseId || '') || 'missing')}</p>`}
<h4>Inputs</h4>${t.inputs.map(a => `<p>${escape(a.name)} — SHA-256 <code>${a.sha256}</code></p>`).join('')}
${candidate.manualRuns.filter(run => run.requirementId === r.id && run.testId === t.id).map(run => `<p><strong>${run.result}</strong> · ${escape(run.author)} · ${escape(run.createdAt)} · ${escape(run.device)}</p>${markdown(run.notes)}${run.evidence.map(a => `<p>Evidence: ${escape(a.name)} — <code>${a.sha256}</code></p>`).join('')}`).join('')}`).join('')}</article>`;
  }).join('')}
<h2>Firmware assets</h2>${candidate.assets.map(a => `<p>${escape(a.name)} — ${a.size} bytes — <code>${a.sha256}</code></p>`).join('')}
<p>Automated run: ${candidate.runId ?? 'not received'} / attempt ${candidate.runAttempt ?? '—'}</p></html>`;
}
