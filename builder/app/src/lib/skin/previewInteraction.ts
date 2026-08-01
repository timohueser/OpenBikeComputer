/** Pointer-session policy for the live preview, kept DOM-free for adversarial tests. */
export class PreviewDragSession {
    #pointerId: number | null = null;
    #x = 0;
    #y = 0;

    get active(): boolean {
        return this.#pointerId !== null;
    }

    begin(pointerId: number, x: number, y: number): boolean {
        if (this.active || !Number.isFinite(x) || !Number.isFinite(y)) return false;
        this.#pointerId = pointerId;
        this.#x = x;
        this.#y = y;
        return true;
    }

    move(pointerId: number, x: number, y: number): { dx: number; dy: number } | null {
        if (pointerId !== this.#pointerId || !Number.isFinite(x) || !Number.isFinite(y)) return null;
        const delta = { dx: x - this.#x, dy: y - this.#y };
        this.#x = x;
        this.#y = y;
        return delta;
    }

    end(pointerId: number): boolean {
        if (pointerId !== this.#pointerId) return false;
        this.cancel();
        return true;
    }

    cancel(): void {
        this.#pointerId = null;
        this.#x = 0;
        this.#y = 0;
    }
}

/** Normalize browser wheel units and cap one event so a trackpad spike cannot teleport scale. */
export function wheelZoomFactor(deltaY: number, deltaMode: number): number {
    const pixels = deltaY * (deltaMode === 1 ? 16 : deltaMode === 2 ? 240 : 1);
    return Math.exp(Math.max(-500, Math.min(500, -pixels)) * 0.0015);
}

/** Combine a trackpad burst for one animation frame without overflowing the wasm boundary. */
export function combineWheelZoom(current: number, next: number): number {
    if (!Number.isFinite(next) || next <= 0) return current;
    return Math.max(1e-6, Math.min(1e6, current * next));
}

export type KeyboardCameraAction =
    | { kind: "pan"; dx: number; dy: number }
    | { kind: "zoom"; factor: number }
    | { kind: "reset" };

export function keyboardCameraAction(key: string): KeyboardCameraAction | null {
    switch (key) {
        case "ArrowLeft":
            return { kind: "pan", dx: 24, dy: 0 };
        case "ArrowRight":
            return { kind: "pan", dx: -24, dy: 0 };
        case "ArrowUp":
            return { kind: "pan", dx: 0, dy: 24 };
        case "ArrowDown":
            return { kind: "pan", dx: 0, dy: -24 };
        case "+":
        case "=":
            return { kind: "zoom", factor: 1.25 };
        case "-":
        case "_":
            return { kind: "zoom", factor: 0.8 };
        case "0":
        case "Home":
            return { kind: "reset" };
        default:
            return null;
    }
}
