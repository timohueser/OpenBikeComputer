/** The GLSL. Every pass reads heights through the same level rule as `Surface` in skyline.ts. */

import { BLANKET } from './skyline';

const LEVELS = /* glsl */ `
precision highp sampler2DArray;
uniform float half0;
uniform float levels;

/** The finest level that holds p; levels or more beyond the last. */
float levelOf(vec2 p) {
  return max(0.0, ceil(log2(max(max(abs(p.x), abs(p.y)), 1.0) / half0)));
}

vec2 uvOn(float k, vec2 p) {
  return vec2(p.x, -p.y) / (2.0 * half0 * exp2(k)) + 0.5;
}
`;

const HEIGHTS = /* glsl */ `
${LEVELS}
uniform sampler2DArray heights;
uniform sampler2DArray canopy;
uniform float dtmLevels;
uniform float canopyLevels;
uniform bool glo30Only;
uniform float lift;
uniform float curvature;

float levelHeight(float k, vec2 p, bool canopied) {
  vec2 uv = uvOn(k, p);
  float layer = glo30Only && k < dtmLevels ? levels + k : k;
  float h = textureLod(heights, vec3(uv, layer), 0.0).r;
  // The canopy blanket, as Surface lays it; canopy bytes are quarter metres.
  if (canopied && !glo30Only && k < canopyLevels)
    h += textureLod(canopy, vec3(uv, k), 0.0).r * 63.75 * smoothstep(${BLANKET[0].toFixed(1)}, ${BLANKET[1].toFixed(1)}, length(p));
  return h;
}

float heightOn(float k, vec2 p, bool canopied) {
  float h = levelHeight(k, p, canopied);
  if (k < levels - 1.0) {
    float w = smoothstep(0.9, 1.0, max(abs(p.x), abs(p.y)) / (half0 * exp2(k)));
    if (w > 0.0) h = mix(h, levelHeight(k + 1.0, p, canopied), w);
  }
  return h;
}

/** The drawn surface: height above the eye, dropped by curvature with refraction. */
float surfaceZ(float k, vec2 p) {
  return heightOn(k, p, true) - lift - curvature * dot(p, p);
}

/** The bare ground under the canopy, which instanced trees stand on. */
float groundZ(float k, vec2 p) {
  return heightOn(k, p, false) - lift - curvature * dot(p, p);
}
`;

/** The polar grid the shadow texture shares with the mesh: texel (j, i) is azimuth j, ring i. */
const GRID = /* glsl */ `
uniform float r0;
uniform float ringStep;
uniform float rings;
uniform float azimuths;
`;

const SHADOW_AT = /* glsl */ `
${GRID}
uniform sampler2D shadows;

float shadowAt(vec2 p) {
  float ring = log(max(length(p), r0) / r0) / ringStep;
  float azimuth = atan(p.x, p.y) / 6.2831853;
  return textureLod(shadows, vec2(azimuth + 0.5 / azimuths, (ring + 0.5) / rings), 0.0).r;
}
`;

const SKY = /* glsl */ `
uniform vec3 zenith;
uniform vec3 horizon;
uniform vec3 sunDir;
uniform float glow;
const vec3 GLOW = vec3(1.0, 0.62, 0.36);

vec3 sky(vec3 dir) {
  float up = clamp(dir.z, 0.0, 1.0);
  vec3 c = mix(horizon, zenith, sqrt(up));
  float toward = max(dot(normalize(dir.xy + 1e-6), normalize(sunDir.xy + 1e-6)), 0.0);
  float near = max(dot(dir, sunDir), 0.0);
  return c + GLOW * glow * (0.35 * pow(toward, 3.0) * exp(-5.0 * up) + 0.5 * pow(near, 24.0));
}
`;

/** Ground colour by level. Reads LEVELS, which the shader brings in before it. */
const COLOUR = /* glsl */ `
uniform sampler2DArray classes;
uniform sampler2DArray s2;
uniform float s2Levels;
uniform vec3 ramps[33];
uniform float snap;
uniform sampler2DArray roads;
uniform float roadLevels;
uniform vec3 roadColours[6];
uniform sampler2DArray waterMap;
uniform float waterLevels;
// Half a level sharper than the pixel footprint.
const float DETAIL = -0.5;

struct Ground {
  vec3 albedo;
  float water;
  float tree;
  float snow;
  float road;
};

float luma(vec3 c) {
  return dot(c, vec3(0.3, 0.59, 0.11));
}

// Dave Hoskins' hash and a smooth value noise over it.
float hash(vec2 q) {
  vec3 p3 = fract(q.xyx * 0.1031);
  p3 += dot(p3, p3.yzx + 33.33);
  return fract((p3.x + p3.y) * p3.z);
}

float noise(vec2 q) {
  vec2 i = floor(q);
  vec2 f = q - i;
  f = f * f * (3.0 - 2.0 * f);
  return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), f.x), mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0)), f.x), f.y);
}

vec3 ramp(int index, float t) {
  vec3 a = ramps[index * 3];
  vec3 b = ramps[index * 3 + 1];
  vec3 c = ramps[index * 3 + 2];
  return t < 0.5 ? mix(a, b, 2.0 * t) : mix(b, c, 2.0 * t - 1.0);
}

Ground groundOn(float k, vec2 p, float footprint) {
  vec2 uv = uvOn(k, p);
  vec4 s = k < s2Levels ? texture(s2, vec3(uv, k)) : vec4(0.0);
  bool seen = s.a > 0.5 / 255.0;
  float ndvi = seen ? (s.a * 255.0 - 1.0) / 127.0 - 1.0 : 0.45;
  float t = clamp((ndvi - 0.15) / 0.6, 0.0, 1.0);
  // The four classes around p, each through its ramp, blended as a bilinear sample would be. A
  // wobble of up to half a texel turns WorldCover's 10 m stairs into brush edges.
  vec2 texel = uv * 1024.0 - 0.5;
  vec2 brushed = texel + 0.45 * (vec2(noise(texel * 0.7), noise(texel * 0.7 + 31.0)) * 2.0 - 1.0);
  ivec2 corner = ivec2(floor(brushed));
  vec2 f = brushed - floor(brushed);
  vec3 palette = vec3(0.0);
  float water = 0.0;
  float tree = 0.0;
  for (int j = 0; j < 4; j++) {
    ivec2 o = ivec2(j & 1, j >> 1);
    vec2 cls = texelFetch(classes, ivec3(clamp(corner + o, ivec2(0), ivec2(1023)), int(k)), 0).rg;
    int index = int(cls.r * 255.0 + 0.5);
    // Unknown ground is painted as grass.
    if (index > 10) index = 2;
    float w = (o.x == 1 ? f.x : 1.0 - f.x) * (o.y == 1 ? f.y : 1.0 - f.y);
    palette += w * ramp(index, t);
    water += w * cls.g;
    tree += index == 0 ? w : 0.0;
  }
  // OSM water: rivers narrower than a WorldCover pixel, and lakes, as coverage from a finer grid.
  if (k < waterLevels) water = max(water, smoothstep(0.4, 0.6, texture(waterMap, vec3(uv, k)).r));
  vec3 albedo = palette;
  float snow = 0.0;
  if (seen) {
    // The satellite gives chroma only: brightness comes from the palette and the sun, because
    // the mosaics carry their own shadows.
    vec3 shown = pow(min(s.rgb * 1.65, vec3(1.0)), vec3(2.2));
    vec3 chroma = shown * (luma(palette) / max(luma(shown), 1e-4));
    albedo = mix(mix(vec3(luma(chroma)), chroma, 0.8), palette, snap);
    vec3 refl = pow(s.rgb, vec3(2.2));
    float bright = smoothstep(0.28, 0.42, min(refl.r, min(refl.g, refl.b)));
    float shaded = step(1.15 * refl.r, refl.b) * step(ndvi, 0.1) * smoothstep(0.03, 0.06, refl.b);
    snow = max(bright, shaded) * (1.0 - water);
  }
  // A road paints where the distance to the nearest road edge is negative; the distance is linear
  // across an edge, so filtering keeps the edge sharp to a pixel. Its surface comes from the four
  // codes around p, blended like the classes, so roads of two kinds meet without a seam.
  float road = 0.0;
  if (k < roadLevels) {
    float edge = texture(roads, vec3(uv, k)).r * 25.5 - 5.0;
    road = 1.0 - smoothstep(-footprint, footprint, edge);
    if (road > 0.0) {
      vec3 surface = vec3(0.0);
      float weight = 0.0;
      ivec2 cell = ivec2(floor(texel));
      vec2 h = texel - floor(texel);
      for (int j = 0; j < 4; j++) {
        ivec2 o = ivec2(j & 1, j >> 1);
        int code = int(texelFetch(roads, ivec3(clamp(cell + o, ivec2(0), ivec2(1023)), int(k)), 0).g * 255.0 + 0.5);
        if (code == 0) continue;
        float w = (o.x == 1 ? h.x : 1.0 - h.x) * (o.y == 1 ? h.y : 1.0 - h.y);
        surface += w * roadColours[code - 1];
        weight += w;
      }
      albedo = mix(albedo, surface / max(weight, 1e-4), road * min(weight * 4.0, 1.0));
    }
  }
  return Ground(albedo, water, tree, snow, road);
}

/**
 * The ground under p from the level whose pixel matches the screen footprint, so the octave levels
 * act as the mip chain, blended into the next one like the heights are.
 */
Ground groundAt(vec2 p, float footprint) {
  float kf = max(levelOf(p), log2(max(footprint / (2.0 * half0 / 1024.0), 1.0)) + DETAIL);
  kf = min(kf, levels - 1.0);
  float k = floor(kf);
  float w = max(kf - k, smoothstep(0.9, 1.0, max(abs(p.x), abs(p.y)) / (half0 * exp2(k))));
  Ground a = groundOn(k, p, footprint);
  if (w <= 0.0 || k >= levels - 1.0) return a;
  Ground b = groundOn(k + 1.0, p, footprint);
  return Ground(mix(a.albedo, b.albedo, w), mix(a.water, b.water, w), mix(a.tree, b.tree, w), mix(a.snow, b.snow, w),
                mix(a.road, b.road, w));
}
`;

export const terrainVertex = /* glsl */ `
${HEIGHTS}
${SHADOW_AT}

out vec3 vWorld;
out float vShadow;

void main() {
  vec2 p = (modelMatrix * vec4(position, 1.0)).xy;
  float k = min(levelOf(p), levels - 1.0);
  vWorld = vec3(p, surfaceZ(k, p));
  vShadow = shadowAt(p);
  gl_Position = projectionMatrix * viewMatrix * vec4(vWorld, 1.0);
}
`;

/**
 * Linear light to the screen: a hue-keeping tone curve (Khronos PBR Neutral), then a grade that
 * leans shadows cool and light warm, then sRGB.
 */
const FINISH = /* glsl */ `
vec3 finish(vec3 c) {
  float low = min(c.r, min(c.g, c.b));
  c -= low < 0.08 ? low - 6.25 * low * low : 0.04;
  float peak = max(c.r, max(c.g, c.b));
  if (peak > 0.76) {
    float top = 1.0 - 0.0576 / (peak - 0.52);
    c *= top / peak;
    c = mix(c, vec3(top), 1.0 - 1.0 / (0.15 * (peak - top) + 1.0));
  }
  float l = dot(c, vec3(0.2126, 0.7152, 0.0722));
  c += (1.0 - smoothstep(0.0, 0.25, l)) * vec3(-0.004, 0.0, 0.012) + smoothstep(0.4, 0.9, l) * vec3(0.01, 0.004, -0.008);
  c = max(mix(vec3(l), c, 1.08), 0.0);
  return mix(12.92 * c, 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055, step(0.0031308, c));
}
`;

/**
 * Light and air, the same for terrain, trees and buildings, in linear light. Reads `curvature`,
 * which the shader declares before it.
 */
const SHADE = /* glsl */ `
${SKY}
${FINISH}
uniform vec3 sunColour;
uniform vec3 ambient;
uniform float softness;
uniform float gradient;
uniform float night;
uniform float eyeAltitude;
uniform float hazeKm;
uniform float hazeHeight;
uniform float mistTop;
uniform sampler2DShadow nearShadow;
uniform mat4 nearShadowMatrix;

// Trees and buildings near the eye cast into a depth map from the sun. Outside it, 1. The
// lookup starts a quarter metre off the surface, so a face does not shadow itself, and the GPU
// blends the four comparisons around it, so an edge is soft rather than stepped.
float nearShadowAt(vec3 world, vec3 n) {
  vec4 q = nearShadowMatrix * vec4(world + 0.25 * n, 1.0);
  vec3 c = q.xyz / q.w * 0.5 + 0.5;
  float reach = max(abs(c.x - 0.5), abs(c.y - 0.5)) * 2.0;
  if (reach > 1.0 || c.z > 1.0) return 1.0;
  // About a tenth of a metre of bias, over the map's 4 km of depth.
  return mix(1.0, texture(nearShadow, vec3(c.xy, c.z - 0.000025)), smoothstep(1.0, 0.9, reach));
}

// Three soft bands of sunlight, with some of the smooth term kept so a high sun still shows
// relief. A face out of the sun keeps the cool sky light, never black; open is how much of
// the sky a hollow still sees.
vec3 lightUp(vec3 albedo, vec3 n, float shadow, float open) {
  float lit = max(dot(n, sunDir), 0.0) * shadow;
  float band = 0.45 * smoothstep(0.03, 0.03 + softness, lit) + 0.55 * smoothstep(0.32, 0.32 + softness, lit);
  return albedo * (ambient * (0.85 + 0.15 * n.z) * open + sunColour * mix(band, lit, gradient));
}

vec3 atmosphere(vec3 c, vec3 world) {
  float d = length(world);
  // Haze lit from behind glows in the sun's colour, most of all when the sun is low.
  float toward = max(dot(world / max(d, 1e-3), sunDir), 0.0);
  vec3 hazeColour = sky(normalize(vec3(world.xy, 0.0)))
    + sunColour * (0.12 + glow) * (0.25 * pow(toward, 6.0) + 0.6 * pow(toward, 40.0));
  // Height fog: the mean of exp(-altitude / H) along the sight line, over the eye's altitude
  // and the true altitude of the fragment.
  float a0 = eyeAltitude;
  float a1 = eyeAltitude + world.z + curvature * dot(world.xy, world.xy);
  float mean = abs(a1 - a0) < 1.0 ? exp(-a0 / hazeHeight)
    : hazeHeight * (exp(-a0 / hazeHeight) - exp(-a1 / hazeHeight)) / (a1 - a0);
  c = mix(c, hazeColour, 1.0 - exp(-d * mean / (hazeKm * 1000.0)));
  // Valley mist: a pale layer below its top, thicker with distance.
  float mist = clamp((mistTop - a1) / 150.0, 0.0, 1.0) * (1.0 - exp(-d / 1500.0));
  c = mix(c, mix(hazeColour, vec3(dot(hazeColour, vec3(0.333)) * 1.15), 0.5), mist);
  // At night the eye loses colour and sees blue.
  c = mix(c, dot(c, vec3(0.3, 0.55, 0.15)) * vec3(0.7, 0.9, 1.5), 0.8 * night);
  return c;
}
`;

export const terrainFragment = /* glsl */ `
${LEVELS}
uniform float curvature;
${SHADE}
${COLOUR}
uniform sampler2DArray normals;
uniform vec3 rock;
uniform vec3 shore;

in vec3 vWorld;
in float vShadow;
out vec4 colour;

/**
 * The normal from the derived slopes, at the level whose texel matches the pixel, so the levels
 * act as its mip chain. The mesh's triangles at range are long along the view, and normals
 * interpolated across them would smear slope into teeth down the fall line.
 */
vec3 groundNormal(vec2 p, float footprint, out float open) {
  float kf = min(max(levelOf(p), log2(max(footprint / (2.0 * half0 / 1024.0), 1.0))), levels - 1.0);
  float k = floor(kf);
  vec4 slope = texture(normals, vec3(uvOn(k, p), k));
  if (kf > k) slope = mix(slope, texture(normals, vec3(uvOn(k + 1.0, p), k + 1.0)), kf - k);
  open = slope.b;
  return normalize(vec3(-slope.rg, 1.0));
}

void main() {
  float footprint = max(length(dFdx(vWorld.xy)), length(dFdy(vWorld.xy)));
  float open;
  vec3 n = groundNormal(vWorld.xy, footprint, open);
  Ground g = groundAt(vWorld.xy, footprint);
  // Steep open ground is rock; trees and roads hold on to it.
  float steep = 1.0 - smoothstep(0.766, 0.809, n.z);
  vec3 albedo = mix(g.albedo, rock, steep * (1.0 - g.tree) * (1.0 - g.road));
  albedo = mix(albedo, ramps[18], g.snow);
  float shadow = vShadow * nearShadowAt(vWorld, n);
  vec3 c = lightUp(albedo, n, shadow, open);

  // Water is the terrain itself, flat on the curved earth: the sky by Fresnel, a sun glint, and
  // a lighter band along the shore. The reflection is held to 60 %, because the far bank it
  // would really show is darker than the sky.
  vec3 dir = normalize(vWorld);
  vec3 up = normalize(vec3(2.0 * curvature * vWorld.xy, 1.0));
  vec3 mirrored = reflect(dir, up);
  float fresnel = 0.6 * (0.02 + 0.98 * pow(1.0 - max(dot(-dir, up), 0.0), 5.0));
  vec3 body = mix(ramps[21], shore, 1.0 - smoothstep(0.5, 0.8, g.water)) * (ambient + 0.5 * sunColour * max(sunDir.z, 0.0));
  vec3 water = mix(body, sky(mirrored), fresnel) + 4.0 * sunColour * shadow * pow(max(dot(mirrored, sunDir), 0.0), 400.0);
  c = mix(c, water, smoothstep(0.45, 0.55, g.water));
  colour = vec4(finish(atmosphere(c, vWorld)), 1.0);
}
`;

/**
 * Instanced trees and plants: a unit model turned to its heading, scaled by its height and stood
 * on the bare ground, in its own vertex colours.
 */
export const treeVertex = /* glsl */ `
${HEIGHTS}
${SHADOW_AT}
${COLOUR}
// How far a model takes its colour from the ground under it: 0 for trees.
uniform float tie;
in vec3 color;
// x, y, height, heading.
in vec4 tree;
in float tint;
out vec3 vWorld;
out vec3 vNormal;
out float vShadow;
out vec3 vAlbedo;

void main() {
  vec2 p = tree.xy;
  mat2 turn = mat2(cos(tree.w), sin(tree.w), -sin(tree.w), cos(tree.w));
  vWorld = vec3(p, groundZ(min(levelOf(p), levels - 1.0), p)) + vec3(turn * position.xy, position.z) * tree.z;
  vNormal = vec3(turn * normal.xy, normal.z);
  vShadow = shadowAt(p);
  vAlbedo = color * tint;
  // A plant takes its hue from the ground it stands on, so a meadow of tufts reads as the meadow;
  // its own colour keeps only the shading from root to tip.
  if (tie > 0.0) vAlbedo = mix(vAlbedo, groundAt(p, 0.5).albedo * clamp(luma(color) / 0.3, 0.4, 1.6) * tint, tie);
  gl_Position = projectionMatrix * viewMatrix * vec4(vWorld, 1.0);
}
`;

/** Buildings: positions in metres above the eye's height, curved like the terrain. */
export const buildingVertex = /* glsl */ `
${SHADOW_AT}
uniform float lift;
uniform float curvature;
in vec3 color;
out vec3 vWorld;
out vec3 vNormal;
out float vShadow;
out vec3 vAlbedo;

void main() {
  vWorld = position - vec3(0.0, 0.0, lift + curvature * dot(position.xy, position.xy));
  vNormal = normal;
  vShadow = shadowAt(position.xy);
  vAlbedo = color;
  gl_Position = projectionMatrix * viewMatrix * vec4(vWorld, 1.0);
}
`;

export const objectFragment = /* glsl */ `
uniform float curvature;
${SHADE}
in vec3 vWorld;
in vec3 vNormal;
in float vShadow;
in vec3 vAlbedo;
out vec4 colour;

void main() {
  vec3 n = normalize(vNormal);
  colour = vec4(finish(atmosphere(lightUp(vAlbedo, n, vShadow * nearShadowAt(vWorld, n), 1.0), vWorld)), 1.0);
}
`;

/**
 * Per level texel: the drawn surface's slope east and north, and how open the ground is. open
 * darkens a crease and lifts a crest a few texels across, like ink in a gully. The eye never
 * moves, so this runs once per surface.
 */
export const deriveFragment = /* glsl */ `
${HEIGHTS}
uniform float deriveLevel;
out vec4 colour;

void main() {
  float k = deriveLevel;
  float e = 2.0 * half0 * exp2(k) / 1024.0;
  vec2 uv = gl_FragCoord.xy / 1024.0;
  vec2 p = vec2(uv.x - 0.5, 0.5 - uv.y) * 2.0 * e * 512.0;
  float east = surfaceZ(k, p + vec2(e, 0.0));
  float west = surfaceZ(k, p - vec2(e, 0.0));
  float north = surfaceZ(k, p + vec2(0.0, e));
  float south = surfaceZ(k, p - vec2(0.0, e));
  float wide = 3.0 * e;
  float around = surfaceZ(k, p + vec2(wide, 0.0)) + surfaceZ(k, p - vec2(wide, 0.0))
    + surfaceZ(k, p + vec2(0.0, wide)) + surfaceZ(k, p - vec2(0.0, wide));
  float open = clamp(1.0 - 1.5 * (0.25 * around - surfaceZ(k, p)) / wide, 0.6, 1.15);
  colour = vec4((east - west) / (2.0 * e), (north - south) / (2.0 * e), open, 1.0);
}
`;

/** Depth only, for the near shadow map; the object's own vertex shader places it. */
export const depthFragment = /* glsl */ `
out vec4 colour;
void main() {
  colour = vec4(1.0);
}
`;

export const fullscreenVertex = /* glsl */ `
out vec2 vNdc;
void main() {
  vNdc = position.xy;
  gl_Position = vec4(position.xy, 1.0, 1.0);
}
`;

export const skyFragment = /* glsl */ `
${SKY}
${FINISH}
uniform mat4 inverseProjection;
uniform mat4 cameraWorld;
uniform vec3 sunColour;
in vec2 vNdc;
out vec4 colour;

void main() {
  vec4 view = inverseProjection * vec4(vNdc, 1.0, 1.0);
  vec3 dir = normalize((cameraWorld * vec4(view.xyz / view.w, 0.0)).xyz);
  float near = max(dot(dir, sunDir), 0.0);
  // A halo around the sun, and a disk about twice the real one.
  vec3 c = sky(dir) + sunColour * (0.06 * pow(near, 16.0) + 0.35 * pow(near, 900.0));
  float disk = smoothstep(0.0095, 0.0075, acos(clamp(dot(dir, sunDir), -1.0, 1.0)));
  c = mix(c, sunColour * 3.0 + 0.3, disk * step(0.001, dot(sunColour, vec3(1.0))));
  colour = vec4(finish(c), 1.0);
}
`;

/**
 * One texel per mesh vertex: march toward the sun in steps that grow from the texel's own scale
 * to 200 km, over the same curved surface. The result is the sun's clearance, softened across
 * about the sun's apparent size.
 */
export const shadowFragment = /* glsl */ `
${HEIGHTS}
${GRID}
uniform vec3 sunDir;
out vec4 colour;
const int STEPS = 96;

void main() {
  float azimuth = (gl_FragCoord.x - 0.5) / azimuths * 6.2831853;
  float r = r0 * exp((gl_FragCoord.y - 0.5) * ringStep);
  vec2 p = r * vec2(sin(azimuth), cos(azimuth));
  float across = length(sunDir.xy);
  if (sunDir.z < -0.2 || across < 1e-4) {
    colour = vec4(sunDir.z > 0.0 ? 1.0 : 0.0, 0.0, 0.0, 1.0);
    return;
  }
  vec2 toward = sunDir.xy / across;
  float slope = sunDir.z / across;
  float k = min(levelOf(p), levels - 1.0);
  float pixel = 2.0 * half0 * exp2(k) / 1024.0;
  float z = surfaceZ(k, p) + 0.25 * pixel;
  float d = max(1.5 * pixel, 0.012 * r);
  float growth = pow(200000.0 / d, 1.0 / float(STEPS - 1));
  float clearance = 1.0;
  for (int i = 0; i < STEPS; i++) {
    vec2 q = p + toward * d;
    float kq = levelOf(q);
    if (kq > levels - 1.0) break;
    clearance = min(clearance, (z + slope * d - surfaceZ(kq, q)) / d);
    d *= growth;
  }
  colour = vec4(smoothstep(-0.006, 0.006, clearance), 0.0, 0.0, 1.0);
}
`;
