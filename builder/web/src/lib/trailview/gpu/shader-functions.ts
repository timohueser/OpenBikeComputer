// For tests: WGSL functions that hold only arithmetic, run in JavaScript as written.

/** The text of a function of a shader, between its braces. */
export function functionOf(source: string, name: string): string {
    const start = source.indexOf(`fn ${name}(`), open = source.indexOf('{', start);
    let at = open + 1;
    for (let depth = 1; depth > 0; at++) depth += source[at] === '{' ? 1 : source[at] === '}' ? -1 : 0;
    return source.slice(open + 1, at - 1);
}

/** Functions of a shader in JavaScript, as written there, with its constants. */
export function shaderFunctions<T>(source: string, names: string[]): T {
    const constants = [...source.matchAll(/^const (\w+): [fu]32 = ([^;]+);/gm)].map(([, name, value]) => `const ${name} = ${value.replace(/\b(0x[0-9a-f]+|\d+)u\b/g, '$1')};`);
    const functions = names.map(name => {
        const args = /\(([^)]*)\)/.exec(source.slice(source.indexOf(`fn ${name}(`)))![1];
        const body = functionOf(source, name).replace(/\blet\b/g, 'const').replace(/\bvar\b/g, 'let').replace(/\b(max|min|abs|round)\(/g, 'Math.$1(').replace(/\b[fu]32\(/g, '(').replace(/(\d)u\b/g, '$1');
        return `function ${name}(${args.replace(/: \w+/g, '')}) {${body}}`;
    });
    const smoothstep = 'function smoothstep(a, b, x) { const t = Math.min(1, Math.max(0, (x - a) / (b - a))); return t * t * (3 - 2 * t); }\nfunction select(a, b, c) { return c ? b : a; }';
    return new Function(`${smoothstep}\n${constants.join('\n')}\n${functions.join('\n')}\nreturn { ${names.join(', ')} };`)();
}
