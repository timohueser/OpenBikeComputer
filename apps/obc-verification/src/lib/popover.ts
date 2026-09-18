/**
 * Placement for a panel that hangs off a control: the test-kind panel, the group list.
 *
 * A panel cannot be drawn below its control by CSS alone. The control can sit anywhere on a long
 * page, and `.workbench` clips what leaves it, so a panel near the bottom is cut off or falls out
 * of the window where nothing can scroll to it. These two pieces put it against the window
 * instead: [`portal`] moves it to the body, which is what makes `position: fixed` mean the window
 * and takes the panel out of the clipping container, and [`place`] computes the box.
 */

/** Distance kept from every window edge, and between a panel and its control. */
const EDGE = 8, OFFSET = 6;

export function portal(node: HTMLElement) {
  document.body.appendChild(node);
  return { destroy: () => node.remove() };
}

/**
 * The style for a panel that is wholly visible: below the control when the room below holds it,
 * above when it does not, and inside both side edges. `max-height` is the room it actually has, so
 * a window too short for the whole panel gives it a scrollbar instead of a part that cannot be
 * reached.
 *
 * Lay the panel out at `width` and let it wrap before calling this. Measuring an unconstrained
 * panel reports the height of text on fewer lines than it will wrap to, which put the test-kind
 * panel about 40 px past the bottom of the window.
 */
export function place(control: HTMLElement, panel: HTMLElement, width: number): string {
  const anchor = control.getBoundingClientRect();
  const below = window.innerHeight - anchor.bottom - EDGE - OFFSET;
  const above = anchor.top - EDGE - OFFSET;
  const room = Math.max(below, above, 0);
  const height = Math.min(panel.scrollHeight, room);
  const top = below >= height ? anchor.bottom + OFFSET : Math.max(EDGE, anchor.top - OFFSET - height);
  const left = Math.min(Math.max(EDGE, anchor.left), window.innerWidth - width - EDGE);
  return `top:${top}px;left:${left}px;width:${width}px;max-height:${room}px`;
}

/** The style a panel is laid out with before it is measured; `place` needs the final width. */
export function sizing(width: number): string { return `top:0;left:0;width:${width}px`; }

/** The widest a panel may be, kept inside the window. */
export function widthFor(preferred: number): number { return Math.min(preferred, window.innerWidth - 2 * EDGE); }
