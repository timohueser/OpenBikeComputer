/**
 * A WGSL file with each `// #include NAME` line replaced by the file NAME relative to its folder, each
 * file once. The engine and `npm run check:wgsl` compose the shaders with it.
 */
export function include(path: string, read: (path: string) => string): string {
    const seen = new Set<string>();
    const compose = (file: string): string => {
        if (seen.has(file)) return '';
        seen.add(file);
        const folder = file.slice(0, file.lastIndexOf('/') + 1);
        return read(file).replace(/^\/\/ #include (\S+)$/gm, (_, name: string) => compose(normal(folder + name)));
    };
    return compose(path);
}

/** A path without `..` parts: `ground/../light/shade.wgsl` is `light/shade.wgsl`. */
function normal(path: string): string {
    const parts: string[] = [];
    for (const part of path.split('/')) {
        if (part === '..') parts.pop();
        else parts.push(part);
    }
    return parts.join('/');
}

/** The includes that a device with `clip-distances` reads in place of the default ones. */
export const CLIP_DISTANCES: Record<string, string> = { 'terrain/unknown.wgsl': 'terrain/unknown-clip.wgsl', 'terrain/water-cut.wgsl': 'terrain/water-cut-clip.wgsl' };
