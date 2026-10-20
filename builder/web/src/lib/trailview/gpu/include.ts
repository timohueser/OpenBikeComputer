/**
 * A WGSL file with each `// #include NAME` line replaced by the file NAME of its folder, each file
 * once. The engine and `npm run check:wgsl` compose the shaders with it.
 */
export function include(path: string, read: (path: string) => string): string {
    const seen = new Set<string>();
    const compose = (file: string): string => {
        if (seen.has(file)) return '';
        seen.add(file);
        const folder = file.slice(0, file.lastIndexOf('/') + 1);
        return read(file).replace(/^\/\/ #include (\S+)$/gm, (_, name: string) => compose(folder + name));
    };
    return compose(path);
}
