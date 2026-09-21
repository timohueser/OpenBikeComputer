/**
 * Placement for a panel that hangs off a control.
 *
 * CSS alone cannot draw a panel below its control: the control can sit anywhere on a long page, and
 * `.workbench` clips what leaves it, so a panel near the bottom is cut off. [`portal`] moves the
 * panel to the body, which makes `position: fixed` mean the window and takes it out of the clipping
 * container, and [`place`] computes the box.
 */

/** Distance kept from every window edge, and between a panel and its control. */
const EDGE = 8, OFFSET = 6;

export function portal(node: HTMLElement) {
  document.body.appendChild(node);
  return { destroy: () => node.remove() };
}

/**
 * The style for a panel that is wholly visible: below the control when there is room, above when
 * there is not, and inside both side edges. `max-height` is the room it has, so a short window
 * gives the panel a scrollbar instead of a part that cannot be reached.
 *
 * Lay the panel out at `width` and let it wrap before calling this: measuring an unconstrained
 * panel reports the height of text on fewer lines than it will wrap to.
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
