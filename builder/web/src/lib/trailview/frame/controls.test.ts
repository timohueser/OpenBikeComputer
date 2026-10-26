// @vitest-environment happy-dom
import { expect, it, vi } from 'vitest';
import { Controls } from './controls';

it('stops reading the keys on the window once its signal aborts', () => {
    const stop = new AbortController(), moved = vi.fn();
    const controls = new Controls(document.createElement('canvas'), { pose: () => ({ east: 0, north: 0, heading: 0, pitch: 0, mode: 'walk', height: 1.6 }), moved, ground: () => 0 }, stop.signal);
    const press = () => {
        const event = new KeyboardEvent('keydown', { key: 'ArrowUp', cancelable: true });
        dispatchEvent(event);
        return event.defaultPrevented;
    };
    expect(press()).toBe(true);
    dispatchEvent(new KeyboardEvent('keyup', { key: 'ArrowUp' }));
    stop.abort();
    expect(press()).toBe(false);
    expect(controls.steer()).toBeUndefined();
    expect(moved).toHaveBeenCalledTimes(1);
});
