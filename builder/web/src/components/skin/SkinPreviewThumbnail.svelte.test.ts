// @vitest-environment happy-dom

import { mount, tick, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SkinPreviewThumbnail from "./SkinPreviewThumbnail.svelte";

describe("SkinPreviewThumbnail", () => {
    let putImageData: ReturnType<typeof vi.fn>;

    beforeEach(() => {
        putImageData = vi.fn();
        vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ putImageData } as never);
    });

    afterEach(() => {
        vi.restoreAllMocks();
        document.body.replaceChildren();
    });

    it("paints the mounted canvas with the rendered frame pixels", async () => {
        const pixels = new Uint8ClampedArray([
            0, 0, 0, 255,
            255, 170, 85, 255,
            85, 170, 255, 255,
            255, 255, 255, 255,
        ]);
        const target = document.createElement("div");
        document.body.append(target);

        const component = mount(SkinPreviewThumbnail, {
            target,
            props: {
                frame: { width: 2, height: 2, pixels },
                label: "Custom skin rendered over Teningen",
            },
        });
        await tick();

        const canvas = target.querySelector("canvas");
        expect(canvas).not.toBeNull();
        expect(canvas?.width).toBe(2);
        expect(canvas?.height).toBe(2);
        expect(putImageData).toHaveBeenCalledOnce();

        const image = putImageData.mock.calls[0]?.[0] as ImageData;
        expect(image.width).toBe(2);
        expect(image.height).toBe(2);
        expect(Array.from(image.data)).toEqual(Array.from(pixels));
        expect(new Set(image.data).size).toBeGreaterThan(1);

        await unmount(component);
    });
});
