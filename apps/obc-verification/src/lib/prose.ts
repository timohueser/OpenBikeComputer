/**
 * Spelling and grammar for the prose fields, from Harper (https://writewithharper.com).
 *
 * Harper runs in a worker on the reader's own computer: no text leaves the browser and there is no
 * account or service behind it. Its WebAssembly binary is large, so it is imported only when a
 * field first asks for a check, and the one worker is shared by every field on the page.
 */
import type { Linter } from 'harper.js';

/** `SuggestionKind` in Harper: 0 replaces the problem text, 1 removes it, 2 adds text after it. */
const REMOVE = 1, INSERT_AFTER = 2;

export type Fix = { label: string; text: string; insert: boolean };
/** One problem in the text, with the offsets it was found at. */
export type Note = { start: number; end: number; source: string; kind: string; message: string; fixes: Fix[] };

let started: Promise<Linter> | undefined;
function linter(): Promise<Linter> {
  started ??= (async () => {
    const [{ WorkerLinter }, { binary }] = await Promise.all([import('harper.js'), import('harper.js/binary')]);
    const worker = new WorkerLinter({ binary });
    await worker.setup();
    return worker;
  })();
  return started;
}

export async function check(text: string, language: 'plaintext' | 'markdown'): Promise<Note[]> {
  const lints = await (await linter()).lint(text, { language });
  return lints.map(lint => {
    const span = lint.span();
    return {
      start: span.start, end: span.end,
      source: lint.get_problem_text(), kind: lint.lint_kind_pretty(), message: lint.message(),
      fixes: lint.suggestions().map(suggestion => {
        const kind = suggestion.kind() as unknown as number, text = suggestion.get_replacement_text();
        return { text, insert: kind === INSERT_AFTER, label: kind === REMOVE ? 'Remove' : text.trim() || 'Add a space' };
      })
    };
  });
}

/** The text with one fix applied. The offsets belong to the text the note was found in, so pass
 *  that same text: an edit since the check moves everything after it. */
export function apply(text: string, note: Note, fix: Fix): string {
  return fix.insert
    ? text.slice(0, note.end) + fix.text + text.slice(note.end)
    : text.slice(0, note.start) + fix.text + text.slice(note.end);
}
