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

/**
 * The declarations of a composed shader that its entry points reach, without comments. A browser parses
 * and validates all text of a shader module when it creates the module, on the one thread that also runs
 * every GPU command of the page; the includes bring in many more functions than a module calls. The
 * directives, the assertions and the overrides stay: a pipeline sets an override by its name.
 */
export function reached(source: string): string {
    const declarations = split(uncommented(source)), named = new Map<string, Declaration>();
    for (const declaration of declarations) if (declaration.name) named.set(declaration.name, declaration);
    const kept = new Set(declarations.filter(declaration => declaration.root)), next = [...kept];
    for (let declaration = next.pop(); declaration; declaration = next.pop()) {
        for (const [word] of declaration.text.matchAll(/[A-Za-z_]\w*/g)) {
            const used = named.get(word);
            if (used && !kept.has(used)) {
                kept.add(used);
                next.push(used);
            }
        }
    }
    return declarations.filter(declaration => kept.has(declaration)).map(declaration => declaration.text).join('\n');
}

interface Declaration { text: string; name?: string; root: boolean }

/** A module-scope declaration's attributes, kind and name; no name for a directive or a `const_assert`. */
const NAME = /^((?:@\w+\s*(?:\([^)]*\))?\s*)*)(fn|struct|alias|const|override|var)\b(?:\s*<[^>]*>)?\s+([A-Za-z_]\w*)/;

/** The module-scope declarations: each ends at a `;` outside brackets, or a function or struct at its closing brace. */
function split(code: string): Declaration[] {
    const declarations: Declaration[] = [];
    let start = 0, depth = 0;
    const end = (until: number) => {
        const text = code.slice(start, until).trim();
        start = until;
        if (!text || text === ';') return;
        const [, attributes = '', kind, name] = NAME.exec(text) ?? [];
        declarations.push({ text, name, root: !name || kind === 'override' || /@(?:vertex|fragment|compute)\b/.test(attributes) });
    };
    for (let at = 0; at < code.length; at++) {
        const c = code[at];
        if (c === '{' || c === '(' || c === '[') depth++;
        else if (c === ')' || c === ']') depth--;
        else if (c === '}' ? --depth === 0 && /\b(?:fn|struct)\b/.test(code.slice(start, code.indexOf('{', start))) : c === ';' && depth === 0) end(at + 1);
    }
    end(code.length);
    return declarations;
}

/** The code without its line comments and its block comments, which nest in WGSL. */
function uncommented(source: string): string {
    const comment = /\/\/|\/\*/g;
    let code = '', at = 0;
    for (let found = comment.exec(source); found; found = comment.exec(source)) {
        code += source.slice(at, found.index);
        if (found[0] === '//') {
            const newline = source.indexOf('\n', found.index);
            at = newline < 0 ? source.length : newline;
        } else {
            let depth = 1;
            for (at = found.index + 2; depth > 0 && at < source.length;) {
                if (source.startsWith('/*', at)) (depth++, (at += 2));
                else if (source.startsWith('*/', at)) (depth--, (at += 2));
                else at++;
            }
        }
        comment.lastIndex = at;
    }
    return code + source.slice(at);
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

/** The includes of the pieces of a junction with a stop arm: their pixels find the mouth (plan/mouth-in.wgsl). */
export const MOUTH: Record<string, string> = { 'roads/corridor-in.wgsl': 'plan/mouth-in.wgsl' };

/** The includes that a device with `clip-distances` reads in place of the default ones. */
export const CLIP_DISTANCES: Record<string, string> = { 'terrain/unknown.wgsl': 'terrain/unknown-clip.wgsl', 'terrain/water-cut.wgsl': 'terrain/water-cut-clip.wgsl' };
