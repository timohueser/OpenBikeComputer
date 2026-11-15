import { look, toggle, VFOV, zoom, type Pose, type Steer } from './camera';

/** What the controls move. */
export interface Camera {
    pose(): Pose;
    /** Sets the pose after an input. */
    moved(pose: Pose): void;
    /** The ground under the eye; undefined while no tile knows it. */
    ground(): number | undefined;
}

/** What the controls tell of the pointer, in CSS pixels of the element: a click, and a move with no button at most once a display frame. */
export interface Pointer {
    onclick?(x: number, y: number): void;
    onhover?(x: number, y: number): void;
}

const KEYS = new Set(['w', 'a', 's', 'd', 'q', 'e', 'arrowup', 'arrowdown', 'arrowleft', 'arrowright', 'shift']);
/** A press looks around once the pointer moves farther than DRAG_PX from it; a release before that and within CLICK_MS is a click. */
const DRAG_PX = 4, CLICK_MS = 300;

/**
 * W A S D or the arrows move, Q and E sink and rise in flight, Shift runs, F walks or flies, a drag looks, the wheel zooms,
 * a short press with no drag clicks.
 * An abort of `signal` removes every listener, also those on the window.
 */
export class Controls {
    private readonly held = new Set<string>();

    constructor(element: HTMLElement, camera: Camera, signal: AbortSignal, { onclick, onhover }: Pointer = {}) {
        // `x` and `y` are where the pointer was at the press, and after the drag starts, at the last move.
        let press: { x: number; y: number; time: number; drag: boolean } | undefined, hover: [number, number] | undefined;
        const local = (event: PointerEvent): [number, number] => {
            const box = element.getBoundingClientRect();
            return [event.clientX - box.left, event.clientY - box.top];
        };
        element.addEventListener('pointerdown', event => {
            press = { x: event.clientX, y: event.clientY, time: event.timeStamp, drag: false };
            element.setPointerCapture(event.pointerId);
        }, { signal });
        element.addEventListener('pointermove', event => {
            if (!press) {
                if (event.buttons || !onhover) return;
                if (!hover) requestAnimationFrame(() => {
                    if (hover && !signal.aborted) onhover(...hover);
                    hover = undefined;
                });
                hover = local(event);
                return;
            }
            press.drag ||= Math.hypot(event.clientX - press.x, event.clientY - press.y) > DRAG_PX;
            if (!press.drag) return;
            camera.moved(look(camera.pose(), event.clientX - press.x, event.clientY - press.y, VFOV / element.clientHeight));
            [press.x, press.y] = [event.clientX, event.clientY];
        }, { signal });
        element.addEventListener('pointerup', event => {
            if (press && !press.drag && event.timeStamp - press.time < CLICK_MS) onclick?.(...local(event));
            press = undefined;
        }, { signal });
        element.addEventListener('wheel', event => {
            event.preventDefault();
            camera.moved(zoom(camera.pose(), event.deltaY, camera.ground()));
        }, { passive: false, signal });
        addEventListener('keydown', event => {
            if (event.ctrlKey || event.metaKey || event.altKey || (event.target as HTMLElement).closest?.('input, select, textarea')) return;
            const key = event.key.toLowerCase();
            if (key === 'f') camera.moved(toggle(camera.pose(), camera.ground()));
            if (!KEYS.has(key)) return;
            event.preventDefault();
            this.held.add(key);
            camera.moved(camera.pose());
        }, { signal });
        addEventListener('keyup', event => this.held.delete(event.key.toLowerCase()), { signal });
        addEventListener('blur', () => this.held.clear(), { signal });
    }

    /** The movement of the held keys, or undefined when none moves the eye. */
    steer(): Steer | undefined {
        const axis = (plus: string[], minus: string[]) => (plus.some(key => this.held.has(key)) ? 1 : 0) - (minus.some(key => this.held.has(key)) ? 1 : 0);
        const steer = { ahead: axis(['w', 'arrowup'], ['s', 'arrowdown']), right: axis(['d', 'arrowright'], ['a', 'arrowleft']), up: axis(['e'], ['q']), run: this.held.has('shift') };
        return steer.ahead || steer.right || steer.up ? steer : undefined;
    }
}
