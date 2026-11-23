/** The motion of the search bar and its panels: short, and with reduced motion a fade or none. */
import { cubicOut } from 'svelte/easing';
import { slide, type TransitionConfig } from 'svelte/transition';

/** The rider asks for less motion. */
export const still = () => matchMedia('(prefers-reduced-motion: reduce)').matches;

/** A panel drops in under the bar; it leaves faster than it comes. */
export function drop(_node: Element, { leave = false } = {}): TransitionConfig {
    const shift = still() ? 0 : leave ? -2 : -4;
    return { duration: leave ? 110 : 160, easing: cubicOut, css: t => `opacity: ${t}; transform: translateY(${(1 - t) * shift}px)` };
}

/** A control opens under its row and closes again. */
export const unfold = (node: Element): TransitionConfig => slide(node, { duration: still() ? 0 : 180, easing: cubicOut });

/** A row under another unfolds with a fade and folds back faster. With reduced motion it only fades. */
export function reveal(node: Element, { leave = false } = {}): TransitionConfig {
    const { css, ...motion } = slide(node, { duration: leave ? 150 : 200, easing: cubicOut });
    return { ...motion, css: (t, u) => `${still() ? '' : `${css!(t, u)};`}opacity: ${t}` };
}

/** A value that is new rises into its place. */
export function rise(_node: Element): TransitionConfig {
    const shift = still() ? 0 : 4;
    return { duration: 240, easing: cubicOut, css: t => `opacity: ${0.2 + 0.8 * t}; transform: translateY(${(1 - t) * shift}px)` };
}

/** A small control appears in its place. */
export function pop(_node: Element): TransitionConfig {
    const from = still() ? 1 : 0.9;
    return { duration: 140, easing: cubicOut, css: t => `opacity: ${t}; transform: scale(${from + (1 - from) * t})` };
}
