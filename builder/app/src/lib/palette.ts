import { platform, type Palette } from "./platform";

export type { Palette };

// An empty grid makes the popover fall back to the OS color picker, which is
// also the right answer on a host that serves no palette at all.
const OS_PICKER: Palette = { columns: 8, colors: [] };

let cached: Promise<Palette> | null = null;

/**
 * The device's 64-color gamut for the picker grid (fetched once, shared).
 *
 * `platform.palette` is non-null exactly on the maintainer editor host, and the color
 * control that calls this sits inside the editor's code-split chunk — so on
 * every host that can reach this module the call is there. The null arm is
 * unreachable rather than a real fallback path, and takes the OS picker instead
 * of throwing because a missing swatch grid is not worth failing a render over.
 */
export function getPalette(): Promise<Palette> {
    const fetchPalette = platform.palette;
    cached ??= fetchPalette
        ? fetchPalette()
              .then((p) => ({ columns: p.columns > 0 ? p.columns : 8, colors: p.colors ?? [] }))
              .catch(() => OS_PICKER)
        : Promise.resolve(OS_PICKER);
    return cached;
}
