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

/**
 * One cloud layer at cloudBase above the eye, curved with the earth like the terrain: fair-weather
 * cells about 2.5 km across with smaller lobes on their edges, drifting with the hour. Reads
 * `curvature` and SKY's sunDir.
 */
const CLOUDS = /* glsl */ `
uniform float cloudCover;
// The noise value where cloud starts, so that cloudCover of the sky is under cloud.
uniform float cloudEdge;
uniform float cloudBase;
uniform float cloudDark;
uniform vec2 cloudDrift;

float cloudHash(vec2 q) {
  vec3 p3 = fract(q.xyx * 0.1031);
  p3 += dot(p3, p3.yzx + 33.33);
  return fract((p3.x + p3.y) * p3.z);
}

float cloudNoise(vec2 q) {
  vec2 i = floor(q);
  vec2 f = q - i;
  f = f * f * (3.0 - 2.0 * f);
  return mix(mix(cloudHash(i), cloudHash(i + vec2(1.0, 0.0)), f.x), mix(cloudHash(i + vec2(0.0, 1.0)), cloudHash(i + vec2(1.0)), f.x), f.y);
}

// How much cloud there is above a point, and how thick it is there, both 0 to 1.
vec2 cloudAt(vec2 xy) {
  if (cloudCover <= 0.0) return vec2(0.0);
  vec2 q = (xy + cloudDrift) / 2500.0;
  float n = 0.0;
  float a = 0.5;
  for (int i = 0; i < 5; i++) {
    n += a * cloudNoise(q);
    q = q * 2.03 + 17.0;
    a *= 0.5;
  }
  return vec2(smoothstep(cloudEdge - 0.04, cloudEdge + 0.06, n), smoothstep(cloudEdge, cloudEdge + 0.25, n));
}

// A cloud's underside: sky light, darker where it is thick; a low sun lights it from the side in
// its own colour, and thin edges toward the sun glow.
vec3 cloudColour(vec3 dir, float thick, vec3 sunLight, vec3 skyLight) {
  float toward = max(dot(dir, sunDir), 0.0);
  float low = 1.0 - smoothstep(-0.05, 0.3, sunDir.z);
  return skyLight * (1.05 - cloudDark * thick) + sunLight * (0.25 + 0.4 * low) * (1.0 - 0.6 * thick)
    + sunLight * pow(toward, 10.0) * (1.0 - thick) * 1.2;
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
uniform sampler2DArray exposureMap;
uniform float waterLevels;
uniform float shoreReach;
uniform vec3 moorColour;
uniform float moorOn;
uniform sampler2DArray habitatMap;
uniform float habitatLevels;
// Per habitat (HABITATS in bake.ts): the kit's colour, and 1 where it has one.
uniform vec4 habitatPaint[13];
// Half a level sharper than the pixel footprint.
const float DETAIL = -0.5;

struct Ground {
  vec3 albedo;
  float water;
  float tree;
  float snow;
  float road;
  // Metres from water to the nearest land; 0 on land.
  float shore;
  // How open the sea is to the swell, 0 sheltered to 1 the open ocean.
  float exposure;
  // Share of open, unfarmed ground (shrub, grass, bare, wetland, moss), and how lush it is, 0–1.
  float rough;
  float lush;
  // Moorland: open ground the satellite sees brown, or heath and bog on a land cover map, 0–1.
  float moor;
  // Exposed rock on a land cover map, 0–1.
  float bare;
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
  float rough = 0.0;
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
    rough += index == 1 || index == 2 || index == 5 || index == 8 || index == 10 ? w : 0.0;
  }
  // OSM water: rivers narrower than a WorldCover pixel, and lakes, as coverage from a finer grid.
  float shore = shoreReach;
  float exposure = 0.5;
  if (k < waterLevels) {
    exposure = texture(exposureMap, vec3(uv, k)).r;
    vec2 osm = texture(waterMap, vec3(uv, k)).rg;
    water = max(water, smoothstep(0.4, 0.6, osm.r));
    shore = osm.g * osm.g * shoreReach;
  }
  vec3 albedo = palette;
  float snow = 0.0;
  if (seen) {
    // The satellite gives chroma only: brightness comes from the palette and the sun, because
    // the mosaics carry their own shadows.
    vec3 shown = pow(min(s.rgb * 1.65, vec3(1.0)), vec3(2.2));
    vec3 chroma = shown * (luma(palette) / max(luma(shown), 1e-4));
    albedo = mix(mix(vec3(luma(chroma)), chroma, 0.8), palette, snap);
    vec3 refl = pow(s.rgb, vec3(2.2));
    // Snow is as blue as it is red; white marl and gypsum are warmer.
    float bright = smoothstep(0.36, 0.5, min(refl.r, min(refl.g, refl.b))) * smoothstep(0.9, 0.97, refl.b / max(refl.r, 1e-3));
    float shaded = step(1.15 * refl.r, refl.b) * step(ndvi, 0.1) * smoothstep(0.03, 0.06, refl.b);
    snow = max(bright, shaded) * (1.0 - water);
  }
  // Moor against pasture: heath and bog reflect as much red as green, grazed grass far more green.
  float moor = 0.0;
  if (seen && moorOn > 0.0) {
    vec3 refl = pow(s.rgb, vec3(2.2));
    moor = rough * (1.0 - water) * (1.0 - smoothstep(1.1, 1.4, refl.g / max(refl.r, 1e-4)));
    albedo = mix(albedo, moorColour, moor);
  }
  // A fine land cover map, where the bake has one, paints its habitats in the kit's colours over
  // the palette, blended across texels like the classes; heath and bog are its moor.
  float bare = 0.0;
  if (k < habitatLevels) {
    vec4 paint = vec4(0.0);
    float mapped = 0.0;
    float wild = 0.0;
    for (int j = 0; j < 4; j++) {
      ivec2 o = ivec2(j & 1, j >> 1);
      int code = int(texelFetch(habitatMap, ivec3(clamp(corner + o, ivec2(0), ivec2(1023)), int(k)), 0).r * 255.0 + 0.5);
      float w = (o.x == 1 ? f.x : 1.0 - f.x) * (o.y == 1 ? f.y : 1.0 - f.y);
      paint += w * habitatPaint[code].a * vec4(habitatPaint[code].rgb, 1.0);
      mapped += code > 0 ? w : 0.0;
      wild += code == 2 || code == 3 ? w : 0.0;
      bare += code == 1 ? w : 0.0;
    }
    albedo = mix(albedo, paint.rgb / max(paint.a, 1e-4), paint.a * (1.0 - water));
    moor = mix(moor, wild, mapped);
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
  return Ground(albedo, water, tree, snow, road, shore, exposure, rough, t, moor, bare);
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
                mix(a.road, b.road, w), mix(a.shore, b.shore, w), mix(a.exposure, b.exposure, w), mix(a.rough, b.rough, w), mix(a.lush, b.lush, w),
                mix(a.moor, b.moor, w), mix(a.bare, b.bare, w));
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
// The grade: what shadows and light lean toward, and the saturation.
uniform vec3 gradeShadow;
uniform vec3 gradeLight;
uniform float saturation;

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
  c += (1.0 - smoothstep(0.0, 0.25, l)) * gradeShadow + smoothstep(0.4, 0.9, l) * gradeLight;
  c = max(mix(vec3(l), c, saturation), 0.0);
  return mix(12.92 * c, 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055, step(0.0031308, c));
}
`;

/**
 * Light and air, the same for terrain, trees and buildings, in linear light. Reads `curvature`,
 * which the shader declares before it.
 */
const SHADE = /* glsl */ `
${SKY}
${CLOUDS}
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
// Wet ground is darker and its roads shine.
uniform float wet;
// The cloud layer drawn once per hour over cloudReach around the eye, for the ground's lookups.
uniform sampler2D cloudMap;
uniform float cloudReach;

vec2 cloudOver(vec2 xy) {
  if (max(abs(xy.x), abs(xy.y)) > cloudReach) return cloudAt(xy);
  return texture(cloudMap, xy / (2.0 * cloudReach) + 0.5).rg;
}

// The share of sunlight that reaches a point through the layer, along the sun's ray.
float cloudShadow(vec3 world) {
  float below = cloudBase - world.z - curvature * dot(world.xy, world.xy);
  if (cloudCover <= 0.0 || below <= 0.0 || sunDir.z <= 0.0) return 1.0;
  vec2 cloud = cloudOver(world.xy + sunDir.xy / max(sunDir.z, 0.08) * below);
  return 1.0 - cloud.x * (0.75 + 0.25 * cloud.y);
}

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

// The haze between the eye (three's cameraPosition, the origin at the spot) and a point.
vec3 atmosphere(vec3 c, vec3 world) {
  vec3 view = world - cameraPosition;
  float d = length(view);
  // Haze lit from behind glows in the sun's colour, most of all when the sun is low.
  float toward = max(dot(view / max(d, 1e-3), sunDir), 0.0);
  vec3 hazeColour = sky(normalize(vec3(view.xy, 0.0)))
    + sunColour * (0.12 + glow) * (0.25 * pow(toward, 6.0) + 0.6 * pow(toward, 40.0));
  // Height fog: the mean of exp(-altitude / H) along the sight line, over the eye's altitude
  // and the true altitude of the fragment.
  float a0 = eyeAltitude + cameraPosition.z;
  float a1 = eyeAltitude + world.z + curvature * dot(world.xy, world.xy);
  float mean = abs(a1 - a0) < 1.0 ? exp(-a0 / hazeHeight)
    : hazeHeight * (exp(-a0 / hazeHeight) - exp(-a1 / hazeHeight)) / (a1 - a0);
  c = mix(c, hazeColour, 1.0 - exp(-d * mean / (hazeKm * 1000.0)));
  // Valley mist: a pale layer below its top, thicker with distance.
  float mist = clamp((mistTop - a1) / 150.0, 0.0, 1.0) * (1.0 - exp(-d / 1500.0));
  c = mix(c, mix(hazeColour, vec3(dot(hazeColour, vec3(0.333)) * 1.15), 0.5), mist);
  // Ground above the cloud base stands in the cloud, where the layer is, and takes its colour.
  float above = a1 - eyeAltitude - cloudBase;
  if (above > -40.0 && cloudCover > 0.0) {
    vec2 cloud = cloudOver(world.xy);
    c = mix(c, cloudColour(view / max(d, 1e-3), cloud.y, sunColour, ambient), smoothstep(-40.0, 160.0, above) * cloud.x);
  }
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
// Below this altitude the satellite's white is rock, spray or cloud, not snow (off season).
uniform float snowline;
// Cliff walls, where no geological map gives the rock's colour.
uniform vec3 cliffRock;
uniform vec3 seaDeep;
uniform vec3 seaShallow;
uniform vec3 heather;
uniform float heatherShare;
uniform float outcrops;
uniform vec3 bedding;
uniform float time;
uniform sampler2DArray rockMap;
uniform float rockLevels;
uniform vec3 variegated[3];

in vec3 vWorld;
in float vShadow;
out vec4 colour;

// Voronoi cells about size metres across: the nearest cell's id hash, the gap to the next cell's
// edge in cell units, and a second hash for its shade.
vec3 cells(vec2 p, float size) {
  vec2 q = p / size;
  vec2 i = floor(q);
  vec2 f = q - i;
  float d1 = 8.0;
  float d2 = 8.0;
  vec2 id = i;
  for (int y = -1; y <= 1; y++) {
    for (int x = -1; x <= 1; x++) {
      vec2 o = vec2(x, y);
      float d = length(o + vec2(hash(i + o), hash(i + o + 17.0)) - f);
      if (d < d1) {
        d2 = d1;
        d1 = d;
        id = i + o;
      } else if (d < d2) {
        d2 = d;
      }
    }
  }
  return vec3(hash(id + 5.0), d2 - d1, hash(id + 9.0));
}

/**
 * The normal from the derived slopes, at the level whose texel matches the pixel, so the levels
 * act as its mip chain. The mesh's triangles at range are long along the view, and normals
 * interpolated across them would smear slope into teeth down the fall line.
 */
vec3 groundNormal(vec2 p, float footprint, float coarser, out float open) {
  float kf = min(max(levelOf(p), log2(max(footprint / (2.0 * half0 / 1024.0), 1.0))) + coarser, levels - 1.0);
  float k = floor(kf);
  vec4 slope = texture(normals, vec3(uvOn(k, p), k));
  if (kf > k) slope = mix(slope, texture(normals, vec3(uvOn(k + 1.0, p), k + 1.0)), kf - k);
  open = slope.b;
  return normalize(vec3(-slope.rg, 1.0));
}

void main() {
  float footprint = max(length(dFdx(vWorld.xy)), length(dFdy(vWorld.xy)));
  float open;
  vec3 n = groundNormal(vWorld.xy, footprint, 0.0, open);
  // How flat the ground is at the pixel's own level: water lies only where it is.
  float level = n.z;
  // A cliff spans few texels across and many metres up, so its fine slopes would streak down
  // the face; it takes its normal from coarser levels instead.
  float cliff = 1.0 - smoothstep(0.4, 0.7, n.z);
  if (cliff > 0.0) {
    float coarseOpen;
    n = normalize(mix(n, groundNormal(vWorld.xy, footprint, 2.0, coarseOpen), cliff));
    open = mix(open, coarseOpen, cliff);
  }
  Ground g = groundAt(vWorld.xy, footprint);
  // Steep open ground is rock, banded in strata by height; trees and roads hold on to it. Where
  // the geological map reaches, the rock takes its unit's colour. Variegated clays erode into
  // badlands, so their beds show on gentler slopes: red, white and green-grey, streaked by gullies.
  // Above sea level, so strata, spray and sea do not move with the eye.
  float altitude = eyeAltitude + vWorld.z + curvature * dot(vWorld.xy, vWorld.xy);
  // Strata follow the bedding planes: level where the kit gives none, tilted where it does.
  float bed = dot(vec3(vWorld.xy, altitude), bedding);
  vec2 strike = length(bedding.xy) > 0.01 ? normalize(vec2(-bedding.y, bedding.x)) * 1.4142 : vec2(1.0);
  float strata = 0.86 + 0.22 * noise(vec2(bed / 9.0, dot(vWorld.xy, strike) / 120.0));
  vec3 stone = rock;
  float banded = 0.0;
  float kr = floor(max(levelOf(vWorld.xy), log2(max(footprint / (2.0 * half0 / 1024.0), 1.0))));
  if (kr < rockLevels) {
    vec4 unit = texture(rockMap, vec3(uvOn(kr, vWorld.xy), kr));
    float across = (vWorld.x - vWorld.y) * 0.7071;
    float layer = 0.7 * noise(vec2(altitude / 4.0, across / 400.0)) + 0.3 * noise(vec2(across / 2.5, altitude / 30.0));
    vec3 beds = mix(mix(variegated[0], variegated[1], smoothstep(0.48, 0.53, layer)), variegated[2], smoothstep(0.58, 0.63, layer));
    banded = unit.a;
    stone = mix(unit.rgb, beds, banded);
  }
  float steep = 1.0 - smoothstep(mix(0.766, 0.8, banded), mix(0.809, 0.88, banded), n.z);
  // Near the eye open ground is mottled, tussocks and bare patches, rather than one flat colour.
  vec3 albedo = g.albedo * (1.0 + (0.22 * noise(vWorld.xy / 2.5) + 0.12 * noise(vWorld.xy / 0.8) - 0.17)
    * (1.0 - g.road) * (1.0 - smoothstep(0.3, 2.0, footprint)));
  // Woodland seen from afar is crowns and the shade between them, darker than its leaves, not one
  // flat colour; past a few metres a pixel the pattern becomes its mean.
  if (g.tree > 0.0) {
    float crowns = footprint < 4.0 ? smoothstep(0.02, 0.3, cells(vWorld.xy, 6.0).y) : 0.55;
    albedo *= mix(1.0, 0.42 + 0.48 * crowns, g.tree * (1.0 - g.road));
  }
  // Heather in patches over the moor.
  float patches = noise(vWorld.xy / 4.0) * 0.55 + noise(vWorld.xy / 23.0) * 0.45;
  albedo = mix(albedo, heather, g.moor * (1.0 - g.road) * smoothstep(1.0 - heatherShare, 1.1 - heatherShare, patches));
  // Rock slabs break through open ground on slopes, ridges and poor grass: cells about 9 m across,
  // each rock or turf, with turf between slabs and a dark rim round each. Too small to see past a
  // few metres a pixel, they become their share of rock colour.
  float rocky = outcrops * g.rough * (1.0 - g.road) * clamp(1.1 * (1.0 - smoothstep(0.78, 0.95, n.z))
    + 0.9 * smoothstep(1.0, 1.08, open) + 0.5 * g.moor - 0.3, 0.0, 1.0)
    * 1.6 * smoothstep(0.3, 0.65, noise(vWorld.xy / 70.0));
  // Mapped exposed rock is outcrops in turf, about a third rock.
  rocky = max(rocky, 0.35 * g.bare);
  // Slab cells only where a pixel can show them; past that, rock is its share of colour.
  if (rocky > 0.0 && footprint >= 4.0) albedo = mix(albedo, stone * 0.92, 0.45 * rocky);
  if (rocky > 0.0 && footprint < 4.0) {
    // Where the rock has a strike, slabs run along it in ribs, three times as long as wide.
    vec2 along = length(bedding.xy) > 0.01 ? normalize(vec2(-bedding.y, bedding.x)) : vec2(1.0, 0.0);
    float ribs = length(bedding.xy) > 0.01 ? 3.0 : 1.0;
    // A wobble of a couple of metres breaks the cells' straight edges into outcrop outlines.
    vec2 wobble = 2.2 * (vec2(noise(vWorld.xy / 4.0), noise(vWorld.xy / 4.0 + 19.0)) - 0.5);
    vec2 q = vWorld.xy + wobble;
    vec3 cell = cells(vec2(dot(q, along) / ribs, dot(q, vec2(-along.y, along.x))), 7.0);
    float slab = step(cell.x, rocky) * smoothstep(0.16, 0.24, cell.y);
    vec3 face = stone * (0.82 + 0.3 * cell.z) * mix(0.8, 1.0, smoothstep(0.24, 0.34, cell.y));
    // Close by, mapped bare rock is gravel between the slabs; the stones on it are models.
    if (g.bare > 0.0 && footprint < 0.5) {
      float grit = 0.78 + 0.3 * noise(vWorld.xy / 0.35) + 0.12 * noise(vWorld.xy / 2.5);
      float close = g.bare * (1.0 - slab) * (1.0 - smoothstep(0.2, 0.5, footprint));
      face = mix(face, stone * vec3(1.02, 1.0, 0.95) * grit, close);
      slab = max(slab, close);
    }
    float near = 1.0 - smoothstep(1.5, 4.0, footprint);
    albedo = mix(mix(albedo, stone * 0.92, 0.45 * rocky), mix(albedo, face, slab), near);
    // Each slab is a facet of its own tilt, so outcrops catch the light and throw their own shade.
    vec2 tilt = vec2(cell.z, fract(cell.x * 7.13)) - 0.5;
    n = normalize(n + vec3(0.9 * tilt, 0.0) * slab * (1.0 - smoothstep(4.0, 12.0, footprint)));
  }
  // Sea cliffs: wet and dark where the waves reach, orange lichen in the spray above.
  vec3 face = (rockLevels > 0.0 ? stone : cliffRock) * strata;
  face *= 1.0 - 0.45 * (1.0 - smoothstep(1.0, 4.0, altitude));
  face = mix(face, vec3(0.42, 0.2, 0.06), 0.28 * smoothstep(3.0, 6.0, altitude) * (1.0 - smoothstep(10.0, 16.0, altitude))
    * smoothstep(0.45, 0.7, noise(vWorld.xy / 3.0 + altitude / 2.0)));
  albedo = mix(albedo, face, steep * (1.0 - g.tree) * (1.0 - g.road));
  float snow = g.snow * smoothstep(snowline - 200.0, snowline, altitude);
  albedo = mix(albedo, ramps[18], snow) * (1.0 - 0.3 * wet * (1.0 - snow));
  float shadow = vShadow * nearShadowAt(vWorld, n) * cloudShadow(vWorld);
  vec3 c = lightUp(albedo, n, shadow, open);

  // Water is the terrain itself, flat on the curved earth: the sky by Fresnel, a sun glint, and
  // a lighter band along the shore. The reflection is held to 60 %, because the far bank it
  // would really show is darker than the sky. A wet road mirrors the sky a little too.
  vec3 dir = normalize(vWorld - cameraPosition);
  vec3 up = normalize(vec3(2.0 * curvature * vWorld.xy, 1.0));
  vec3 mirrored = reflect(dir, up);
  float fresnel = 0.6 * (0.02 + 0.98 * pow(1.0 - max(dot(-dir, up), 0.0), 5.0));
  c += wet * g.road * fresnel * sky(mirrored);
  // The body turns from the shallow colour beside the shore, where the surf churns it, to the deep
  // one within a few tens of metres: rocky coasts drop off fast.
  float sea = 1.0 - smoothstep(1.0, 3.0, altitude);
  vec3 body = mix(seaDeep, mix(seaShallow, seaDeep, smoothstep(2.0, 22.0, g.shore)), sea) * (ambient + 0.8 * sunColour * max(sunDir.z, 0.0));
  // Swell rolls in as faint lighter bands, and the sun's path glints in sparks rather than a sheen.
  body *= 1.0 + 0.06 * sin(dot(vWorld.xy, vec2(0.8, 0.6)) / 9.0 + time * 0.9) * (1.0 - smoothstep(2.0, 8.0, footprint));
  float sparks = smoothstep(0.8, 0.86, noise(vWorld.xy / 1.7 + vec2(time * 0.7, time * 0.3)));
  vec3 water = mix(body, sky(mirrored), 0.35 * fresnel)
    + sunColour * shadow * (4.0 * pow(max(dot(mirrored, sunDir), 0.0), 400.0) + 2.5 * sparks * pow(max(dot(mirrored, sunDir), 0.0), 80.0));
  // Surf on the sea: a white rim at the shore, at least a pixel wide so a far coast keeps its line,
  // and broken crests rolling in. Shores open to the swell get a wide rim and crests far out;
  // sheltered ones a thin line.
  float swell = smoothstep(0.05, 0.8, g.exposure);
  float rim = 1.0 - smoothstep(1.5, (3.5 + 3.0 * noise(vWorld.xy / 3.0)) * (0.6 + 1.6 * swell), g.shore / max(footprint / 2.5, 1.0));
  float crest = smoothstep(0.84, 0.94, fract(g.shore / 17.0 + time * 0.05)) * (1.0 - smoothstep(8.0, 15.0 + 70.0 * swell, g.shore))
    * smoothstep(0.35, 0.6, noise(vWorld.xy / 6.0 + vec2(time * 0.2, 0.0))) * (1.0 - smoothstep(1.0, 4.0, footprint)) * (0.2 + 0.8 * swell);
  water = mix(water, vec3(0.85) * (ambient + sunColour * max(sunDir.z, 0.0) * shadow), sea * max(rim, crest));
  c = mix(c, water, smoothstep(0.45, 0.55, g.water) * smoothstep(0.75, 0.9, level));
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
// Plants that grow only on moor (1) or only off it (-1); 0 grows anywhere.
uniform float habitat;
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
  if (tie > 0.0 || habitat != 0.0) {
    Ground g = groundAt(p, 0.5);
    // Shaded from root to tip around the ground's own brightness, never much brighter than it.
    vAlbedo = mix(vAlbedo, g.albedo * clamp(luma(color) / 0.3, 0.5, 1.2) * tint, tie);
    // Off its habitat a plant folds to a point under the ground.
    if (moorOn > 0.0 && habitat * (g.moor - 0.5) < 0.0) vWorld = vec3(p, vWorld.z - 50.0);
  }
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
  float shadow = vShadow * nearShadowAt(vWorld, n) * cloudShadow(vWorld);
  colour = vec4(finish(atmosphere(lightUp(vAlbedo * (1.0 - 0.2 * wet), n, shadow, 1.0), vWorld)), 1.0);
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
uniform float curvature;
${CLOUDS}
${FINISH}
uniform mat4 inverseProjection;
uniform mat4 cameraWorld;
uniform vec3 sunColour;
uniform vec3 ambient;
uniform float hazeKm;
uniform float towers;
in vec2 vNdc;
out vec4 colour;

/**
 * Cumulus towers on the horizon: flat grey bases a little above it, cauliflower tops lit from the
 * sun's side. A silhouette per azimuth, so they stand in groups with sky between. Returns the
 * colour and how much of it covers the sky.
 */
vec4 tower(vec3 dir, vec3 sunLight, vec3 skyLight) {
  // Noise on the unit circle of the bearing, so it has no seam where the angle wraps.
  vec2 h = normalize(dir.xy + 1e-6);
  float e = asin(clamp(dir.z, -1.0, 1.0));
  float group = smoothstep(0.48, 0.68, cloudNoise(h * 6.5 + 3.0));
  float rise = group * (0.04 + 0.12 * cloudNoise(h * 11.0 + 7.0)) * towers;
  float lumps = 0.009 * (cloudNoise(h * 70.0 + e * 30.0) + 0.4 * cloudNoise(h * 160.0 + e * 60.0));
  float base = 0.006 + 0.01 * cloudNoise(h * 12.0 + 1.0);
  float cover = smoothstep(base - 0.001, base + 0.002, e) * (1.0 - smoothstep(rise + lumps - 0.004, rise + lumps, e - base));
  if (cover <= 0.0 || rise <= 0.0) return vec4(0.0);
  float up = clamp((e - base) / max(rise, 1e-3), 0.0, 1.0);
  float side = max(dot(normalize(dir.xy + 1e-6), normalize(sunDir.xy + 1e-6)), 0.0);
  float puff = smoothstep(0.35, 0.65, cloudNoise(h * 110.0 + e * 70.0));
  vec3 c = skyLight * (0.75 + 0.35 * up) + sunLight * (0.35 + 0.45 * up + 0.25 * puff) * (0.7 + 0.3 * side);
  return vec4(c, cover);
}

void main() {
  vec4 view = inverseProjection * vec4(vNdc, 1.0, 1.0);
  vec3 dir = normalize((cameraWorld * vec4(view.xyz / view.w, 0.0)).xyz);
  float near = max(dot(dir, sunDir), 0.0);
  // A halo around the sun, and a disk about twice the real one.
  vec3 c = sky(dir) + sunColour * (0.06 * pow(near, 16.0) + 0.35 * pow(near, 900.0));
  float disk = smoothstep(0.0095, 0.0075, acos(clamp(dot(dir, sunDir), -1.0, 1.0)));
  c = mix(c, sunColour * 3.0 + 0.3, disk * step(0.001, dot(sunColour, vec3(1.0))));
  // The view ray meets the cloud base, which drops away with the earth's curve, so the layer runs
  // on to the horizon and fades into the haze there.
  float across = curvature * dot(dir.xy, dir.xy);
  float base = max(cloudBase - cameraPosition.z, 1.0);
  float t = 2.0 * base / (dir.z + sqrt(dir.z * dir.z + 4.0 * across * base));
  vec2 cloud = cloudAt(cameraPosition.xy + dir.xy * t);
  float seen = cloud.x * exp(-t / (hazeKm * 1500.0));
  c = mix(c, cloudColour(dir, cloud.y, sunColour, ambient), seen);
  if (towers > 0.0) {
    vec4 t = tower(dir, sunColour, ambient);
    // Far off, the towers sit behind haze.
    c = mix(c, mix(t.rgb, horizon, 0.25), t.a);
  }
  colour = vec4(finish(c), 1.0);
}
`;

/** The cloud layer over cloudReach around the eye, one texel per 78 m: cover and thickness. */
export const cloudMapFragment = /* glsl */ `
${SKY}
uniform float curvature;
${CLOUDS}
uniform float cloudReach;
out vec4 colour;

void main() {
  vec2 xy = (gl_FragCoord.xy / 1024.0 - 0.5) * 2.0 * cloudReach;
  colour = vec4(cloudAt(xy), 0.0, 1.0);
}
`;

/**
 * Rain over the view: three layers of slanted streaks, nearer ones larger and faster, in the
 * colour of the sky light.
 */
export const rainFragment = /* glsl */ `
${FINISH}
uniform float rain;
uniform float time;
uniform vec3 ambient;
in vec2 vNdc;
out vec4 colour;

float rainHash(vec2 q) {
  vec3 p3 = fract(q.xyx * 0.1031);
  p3 += dot(p3, p3.yzx + 33.33);
  return fract((p3.x + p3.y) * p3.z);
}

void main() {
  vec2 uv = vNdc * 0.5 + 0.5;
  float a = 0.0;
  for (int i = 0; i < 3; i++) {
    float s = float(i);
    vec2 q = vec2(uv.x * (140.0 - 40.0 * s) + uv.y * 5.0, uv.y * (5.0 - 1.2 * s) + time * (1.6 + 0.9 * s));
    float h = rainHash(vec2(floor(q.x), s));
    float y = fract(q.y + h * 13.0);
    float x = fract(q.x) - 0.5;
    a += smoothstep(0.0, 0.06, y) * smoothstep(0.3, 0.08, y) * smoothstep(0.14, 0.0, abs(x)) * step(0.6, h) * (0.5 + 0.3 * s);
  }
  colour = vec4(finish(ambient * 1.4), min(a, 1.0) * 0.3 * rain);
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
