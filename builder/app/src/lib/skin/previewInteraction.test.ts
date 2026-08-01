import { describe, expect, it } from "vitest";

import { keyboardCameraAction, PreviewDragSession, wheelZoomFactor } from "./previewInteraction";

describe("PreviewDragSession", () => {
    it("owns exactly one pointer and reports incremental deltas", () => {
        const drag = new PreviewDragSession();
        expect(drag.begin(7, 10, 20)).toBe(true);
        expect(drag.begin(8, 0, 0), "a second finger cannot steal the drag").toBe(false);
        expect(drag.move(8, 50, 50), "foreign pointer moves are ignored").toBeNull();
        expect(drag.move(7, 13, 18)).toEqual({ dx: 3, dy: -2 });
        expect(drag.move(7, 14, 22)).toEqual({ dx: 1, dy: 4 });
    });

    it("cleans up on end and cancel without accepting a stale pointer", () => {
        const drag = new PreviewDragSession();
        drag.begin(4, 1, 2);
        expect(drag.end(99)).toBe(false);
        expect(drag.active).toBe(true);
        expect(drag.end(4)).toBe(true);
        expect(drag.active).toBe(false);
        expect(drag.move(4, 5, 6)).toBeNull();

        drag.begin(5, 3, 4);
        drag.cancel();
        expect(drag.active).toBe(false);
        expect(drag.begin(6, Number.NaN, 0), "invalid coordinates never wedge a session").toBe(false);
        expect(drag.begin(6, 0, 0)).toBe(true);
    });
});

describe("preview camera controls", () => {
    it("normalizes wheel units, preserves direction, and bounds hostile deltas", () => {
        expect(wheelZoomFactor(-1, 0)).toBeGreaterThan(1);
        expect(wheelZoomFactor(1, 0)).toBeLessThan(1);
        expect(wheelZoomFactor(-1, 1)).toBeCloseTo(wheelZoomFactor(-16, 0));
        expect(wheelZoomFactor(-1, 2)).toBeCloseTo(wheelZoomFactor(-240, 0));
        expect(wheelZoomFactor(-Infinity, 0)).toBeCloseTo(Math.exp(0.75));
        expect(wheelZoomFactor(Infinity, 0)).toBeCloseTo(Math.exp(-0.75));
    });

    it("maps only documented keys to camera actions", () => {
        expect(keyboardCameraAction("ArrowRight")).toEqual({ kind: "pan", dx: -24, dy: 0 });
        expect(keyboardCameraAction("+")).toEqual({ kind: "zoom", factor: 1.25 });
        expect(keyboardCameraAction("Home")).toEqual({ kind: "reset" });
        expect(keyboardCameraAction("Escape")).toBeNull();
    });
});
