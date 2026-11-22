/** The line that the list and the map light together: from a pointer on a row or a line, else from a row that a key focused. */

export type Side = 'list' | 'map';

export class HoverState {
    /** The line under the pointer, and the side the pointer is on. The pointer can be on the list with no row under it. */
    pointerId = $state<string>();
    pointerFrom = $state<Side>();
    /** The row that a key focused. */
    focusId = $state<string>();
    /** The pointer wins over the focus. Effects that read it run again only when the lit line changes. */
    readonly id = $derived(this.pointerId ?? this.focusId);

    point(id: string | undefined, from: Side) {
        this.pointerId = id;
        this.pointerFrom = from;
    }

    /** Only the side that the pointer is on clears it: a move of the map under a pointer on the list keeps the row lit. */
    leave(from: Side) {
        if (this.pointerFrom !== from) return;
        this.pointerId = this.pointerFrom = undefined;
    }

    focus(id: string) {
        this.focusId = id;
    }

    blur() {
        this.focusId = undefined;
    }

    /** The list moved under a still pointer: `find` gives the line of the row under it now. */
    lookup(find: () => string | undefined) {
        if (this.pointerFrom === 'list') this.pointerId = find();
    }
}
