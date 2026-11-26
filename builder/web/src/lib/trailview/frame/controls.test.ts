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

it('clicks on a short press, and looks around only once a drag leaves the press by more than 4 pixels', () => {
    const canvas = document.createElement('canvas'), moved = vi.fn(), onclick = vi.fn();
    Object.defineProperty(canvas, 'clientHeight', { value: 600 });
    new Controls(canvas, { pose: () => ({ east: 0, north: 0, heading: 0, pitch: 0, mode: 'walk', height: 1.6 }), moved, ground: () => 0 }, new AbortController().signal, { onclick });
    const pointer = (type: string, x: number, y: number, button = 0) => canvas.dispatchEvent(new PointerEvent(type, { clientX: x, clientY: y, pointerId: 1, button, buttons: type === 'pointerup' ? 0 : 1 }));
    pointer('pointerdown', 100, 100);
    pointer('pointermove', 103, 102);
    pointer('pointerup', 103, 102);
    expect(onclick).toHaveBeenCalledExactlyOnceWith(103, 102);
    expect(moved).not.toHaveBeenCalled();

    pointer('pointerdown', 100, 100);
    pointer('pointermove', 104, 100);
    expect(moved).not.toHaveBeenCalled();
    pointer('pointermove', 106, 100);
    // The look takes the whole way from the press: 6 pixels of the 60° over 600 pixels.
    expect(moved).toHaveBeenCalledOnce();
    expect(moved.mock.calls[0][0].heading).toBeCloseTo(359.4, 9);
    pointer('pointerup', 106, 100);
    expect(onclick).toHaveBeenCalledOnce();

    // The right button looks around from the first pixel and never clicks.
    pointer('pointerdown', 100, 100, 2);
    pointer('pointermove', 101, 100, 2);
    pointer('pointerup', 101, 100, 2);
    expect(moved).toHaveBeenCalledTimes(2);
    expect(onclick).toHaveBeenCalledOnce();
});
