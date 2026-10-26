import { look, toggle, VFOV, zoom, type Pose, type Steer } from './camera';

/** What the controls move. */
export interface Camera {
    pose(): Pose;
    /** Sets the pose after an input. */
    moved(pose: Pose): void;
    /** The ground under the eye; undefined while no tile knows it. */
    ground(): number | undefined;
}

const KEYS = new Set(['w', 'a', 's', 'd', 'q', 'e', 'arrowup', 'arrowdown', 'arrowleft', 'arrowright', 'shift']);

/**
 * W A S D or the arrows move, Q and E sink and rise in flight, Shift runs, F walks or flies, a drag looks, the wheel zooms.
 * An abort of `signal` removes every listener, also those on the window.
 */
export class Controls {
    private readonly held = new Set<string>();

    constructor(element: HTMLElement, camera: Camera, signal: AbortSignal) {
        let drag: { x: number; y: number } | undefined;
        element.addEventListener('pointerdown', event => {
            drag = { x: event.clientX, y: event.clientY };
            element.setPointerCapture(event.pointerId);
        }, { signal });
        element.addEventListener('pointermove', event => {
            if (!drag) return;
            camera.moved(look(camera.pose(), event.clientX - drag.x, event.clientY - drag.y, VFOV / element.clientHeight));
            drag = { x: event.clientX, y: event.clientY };
        }, { signal });
        element.addEventListener('pointerup', () => (drag = undefined), { signal });
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
