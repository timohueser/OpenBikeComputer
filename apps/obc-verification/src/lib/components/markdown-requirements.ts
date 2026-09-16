import type { Requirement } from '$lib/types';

export interface ParsedRequirement { id: string; title: string; statement: string; group?: string }

/** Parse `## Group` headings and `- **ID — Title.** Statement` entries. Other lines are ignored. */
export function parseRequirements(text: string): ParsedRequirement[] {
  const result: ParsedRequirement[] = [];
  let group = '';
  for (const line of text.split(/\r?\n/)) {
    const heading = /^##\s+(.+?)\s*$/.exec(line);
    if (heading) { group = heading[1]; continue; }
    const entry = /^[-*]\s+\*\*([A-Za-z0-9][A-Za-z0-9._-]*)\s+[—–-]\s+(.+?)\.?\*\*\s+(.+?)\s*$/.exec(line);
    if (entry) result.push({ id: entry[1], title: entry[2], statement: entry[3], ...(group ? { group } : {}) });
  }
  return result;
}

export function formatRequirements(requirements: Requirement[]): string {
  const groups = [...new Set(requirements.map(r => r.group?.trim() || ''))];
  return groups.map(group => {
    const entries = requirements.filter(r => (r.group?.trim() || '') === group).map(r => `- **${r.id} — ${r.title.replace(/\.$/, '')}.** ${r.statement.replace(/\s*\n+\s*/g, ' ')}`);
    return `## ${group || 'Ungrouped'}\n\n${entries.join('\n')}\n`;
  }).join('\n');
}
