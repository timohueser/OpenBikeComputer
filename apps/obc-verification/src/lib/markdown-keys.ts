/**
 * What the Enter key does inside a Markdown list: carry the list on. `- item` starts `- `, `3. item`
 * starts `4. `, and a task item starts an empty box. Enter on an item with no text ends the list.
 *
 * The result is an edit rather than a new text, so the caller can apply it with the browser's own
 * editing command and keep undo working.
 */
export interface Edit { from: number; to: number; text: string }

/** Indent, marker, space, optional task box, and the text of the item. */
const ITEM = /^(\s*)([-*+]|\d+[.)])(\s+)(\[[ xX]\]\s+)?(.*)$/;

export function listEnter(value: string, at: number): Edit | undefined {
  const start = value.lastIndexOf('\n', at - 1) + 1;
  const wrap = value.indexOf('\n', at);
  const end = wrap === -1 ? value.length : wrap;
  const item = ITEM.exec(value.slice(start, end));
  if (!item) return;
  const [, indent, marker, space, task, content] = item;
  // Inside the marker itself, Enter is just Enter.
  if (at < start + indent.length + marker.length + space.length + (task?.length ?? 0)) return;
  // An item with nothing in it ends the list.
  if (!content.trim() && !task?.includes('x') && !task?.includes('X')) return { from: start, to: end, text: '' };
  const ordered = /^\d+/.exec(marker);
  const next = ordered ? `${Number(ordered[0]) + 1}${marker.slice(ordered[0].length)}` : marker;
  return { from: at, to: at, text: `\n${indent}${next}${space}${task ? '[ ] ' : ''}` };
}
