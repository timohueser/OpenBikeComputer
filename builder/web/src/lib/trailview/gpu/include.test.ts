import { describe, expect, it } from 'vitest';
import { reached } from './include';

describe('the declarations that a shader reaches', () => {
    it('keeps what the entry points use, the directives and the overrides, and drops the rest and the comments', () => {
        const source = `enable clip_distances;
// A comment with /* and a fn unused() { } in it.
/* A block comment /* nested */ that spans
   lines. */
override SHOWN: f32 = 1.0;
const HALF = 0.5;
const UNUSED = 2.0;
struct Out { @builtin(position) at: vec4f, shade: f32 }
@group(0) @binding(0) var<uniform> frame: vec4f;
@group(0) @binding(1) var<storage, read> spare: array<f32>;
fn half(x: f32) -> f32 { return x * HALF; }
fn unused(x: f32) -> f32 { return half(x) + UNUSED + spare[0]; }
@vertex
fn main(@builtin(vertex_index) v: u32) -> Out {
    var out: Out;
    out.at = frame;
    out.shade = half(f32(v)) * SHOWN;
    return out;
}`;
        expect(reached(source).split('\n')).toEqual([
            'enable clip_distances;',
            'override SHOWN: f32 = 1.0;',
            'const HALF = 0.5;',
            'struct Out { @builtin(position) at: vec4f, shade: f32 }',
            '@group(0) @binding(0) var<uniform> frame: vec4f;',
            'fn half(x: f32) -> f32 { return x * HALF; }',
            '@vertex',
            'fn main(@builtin(vertex_index) v: u32) -> Out {',
            '    var out: Out;',
            '    out.at = frame;',
            '    out.shade = half(f32(v)) * SHOWN;',
            '    return out;',
            '}',
        ]);
    });
});
