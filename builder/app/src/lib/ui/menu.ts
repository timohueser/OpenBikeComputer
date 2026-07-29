/**
 * The `<details>`-menu closer: a `<details>` does not close itself when an item is picked, and the
 * "Keep on device" submenu means the picked button can sit two `<details>` deep. Close the whole
 * ancestor chain, then run the action.
 */
export function menuPick(event: Event, action: () => void): void {
    let details = (event.currentTarget as HTMLElement).closest("details");
    while (details) {
        details.removeAttribute("open");
        details = details.parentElement?.closest("details") ?? null;
    }
    action();
}
