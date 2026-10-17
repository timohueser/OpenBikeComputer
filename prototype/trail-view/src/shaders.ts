/** The GLSL. Every pass reads heights through the same level rule as `Surface` in skyline.ts. */

const HEIGHTS = /* glsl */ `
precision highp sampler2DArray;
uniform sampler2DArray heights;
uniform float half0;
uniform float levels;
uniform float dtmLevels;
uniform bool glo30Only;
uniform float lift;
uniform float curvature;

float levelHeight(float k, vec2 p) {
  float half_ = half0 * exp2(k);
  vec2 uv = vec2(p.x, -p.y) / (2.0 * half_) + 0.5;
  float layer = glo30Only && k < dtmLevels ? levels + k : k;
  return textureLod(heights, vec3(uv, layer), 0.0).r;
}

float heightOn(float k, vec2 p) {
  float h = levelHeight(k, p);
  if (k < levels - 1.0) {
    float w = smoothstep(0.9, 1.0, max(abs(p.x), abs(p.y)) / (half0 * exp2(k)));
    if (w > 0.0) h = mix(h, levelHeight(k + 1.0, p), w);
  }
  return h;
}

/** The finest level that holds p; levels or more beyond the last. */
float levelOf(vec2 p) {
  return max(0.0, ceil(log2(max(max(abs(p.x), abs(p.y)), 1.0) / half0)));
}

/** The drawn surface: height above the eye, dropped by curvature with refraction. */
float surfaceZ(float k, vec2 p) {
  return heightOn(k, p) - lift - curvature * dot(p, p);
}
`;

/** The polar grid the shadow texture shares with the mesh: texel (j, i) is azimuth j, ring i. */
const GRID = /* glsl */ `
uniform float r0;
uniform float ringStep;
uniform float rings;
uniform float azimuths;
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

export const terrainVertex = /* glsl */ `
${HEIGHTS}
${GRID}
uniform sampler2D shadows;

out vec3 vWorld;
out vec3 vNormal;
out float vInside;
out float vShadow;

float shadowAt(vec2 p) {
  float ring = log(max(length(p), r0) / r0) / ringStep;
  float azimuth = atan(p.x, p.y) / 6.2831853;
  return textureLod(shadows, vec2(azimuth + 0.5 / azimuths, (ring + 0.5) / rings), 0.0).r;
}

void main() {
  vec2 p = (modelMatrix * vec4(position, 1.0)).xy;
  float r = length(p);
  float k = levelOf(p);
  vInside = k < levels ? 1.0 : 0.0;
  k = min(k, levels - 1.0);
  // The normal at the mesh's own scale, over the curved surface, so it tilts with the local
  // vertical at range.
  float e = max(2.0 * half0 * exp2(k) / 1024.0, 0.0103 * r);
  float dx = surfaceZ(k, p + vec2(e, 0.0)) - surfaceZ(k, p - vec2(e, 0.0));
  float dy = surfaceZ(k, p + vec2(0.0, e)) - surfaceZ(k, p - vec2(0.0, e));
  vNormal = normalize(vec3(-dx, -dy, 2.0 * e));
  vWorld = vec3(p, surfaceZ(k, p));
  vShadow = shadowAt(p);
  gl_Position = projectionMatrix * viewMatrix * vec4(vWorld, 1.0);
}
`;

export const terrainFragment = /* glsl */ `
${SKY}
uniform vec3 ground;
uniform vec3 sunColour;
uniform vec3 ambient;
uniform float softness;
uniform float gradient;
uniform float night;
uniform float eyeAltitude;
uniform float curvature;
uniform float hazeKm;
uniform float hazeHeight;
uniform float mistTop;

in vec3 vWorld;
in vec3 vNormal;
in float vInside;
in float vShadow;
out vec4 colour;

void main() {
  if (vInside < 0.5) discard;
  vec3 n = normalize(vNormal);
  // Three soft bands of sunlight, with some of the smooth term kept so a high sun still shows
  // relief. A face out of the sun keeps the cool sky light, never black.
  float lit = max(dot(n, sunDir), 0.0) * vShadow;
  float band = 0.45 * smoothstep(0.03, 0.03 + softness, lit) + 0.55 * smoothstep(0.32, 0.32 + softness, lit);
  vec3 c = ground * (ambient * (0.65 + 0.35 * n.z) + sunColour * mix(band, lit, gradient));

  float d = length(vWorld);
  vec3 dir = vWorld / d;
  vec3 hazeColour = sky(normalize(vec3(dir.xy, 0.0)));
  // Height fog: the mean of exp(-altitude / H) along the sight line, over the eye's altitude
  // and the true altitude of the fragment.
  float a0 = eyeAltitude;
  float a1 = eyeAltitude + vWorld.z + curvature * dot(vWorld.xy, vWorld.xy);
  float mean = abs(a1 - a0) < 1.0 ? exp(-a0 / hazeHeight)
    : hazeHeight * (exp(-a0 / hazeHeight) - exp(-a1 / hazeHeight)) / (a1 - a0);
  c = mix(c, hazeColour, 1.0 - exp(-d * mean / (hazeKm * 1000.0)));
  // Valley mist: a pale layer below its top, thicker with distance.
  float mist = clamp((mistTop - a1) / 150.0, 0.0, 1.0) * (1.0 - exp(-d / 1500.0));
  vec3 mistColour = mix(hazeColour, vec3(dot(hazeColour, vec3(0.333)) * 1.15), 0.5);
  c = mix(c, mistColour, mist);
  // At night the eye loses colour and sees blue.
  c = mix(c, dot(c, vec3(0.3, 0.55, 0.15)) * vec3(0.7, 0.9, 1.5), 0.8 * night);
  colour = vec4(min(c, 1.0), 1.0);
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
uniform mat4 inverseProjection;
uniform mat4 cameraWorld;
uniform vec3 sunColour;
in vec2 vNdc;
out vec4 colour;

void main() {
  vec4 view = inverseProjection * vec4(vNdc, 1.0, 1.0);
  vec3 dir = normalize((cameraWorld * vec4(view.xyz / view.w, 0.0)).xyz);
  vec3 c = sky(dir);
  // A sun disk about twice the real one.
  float disk = smoothstep(0.0095, 0.0075, acos(clamp(dot(dir, sunDir), -1.0, 1.0)));
  c = mix(c, min(sunColour * 1.4 + 0.25, vec3(1.0)), disk * step(0.001, dot(sunColour, vec3(1.0))));
  colour = vec4(min(c, 1.0), 1.0);
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
