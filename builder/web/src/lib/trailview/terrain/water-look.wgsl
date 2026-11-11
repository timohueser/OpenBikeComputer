// The look of the water's surface, which the water meshes (water.wgsl) and the ground plan's water faces
// (roads/corridor.wgsl) share: a still body, dark from above and a mirror of the sky toward the horizon, with a
// sun glint, small waves near the eye, and foam where the surface falls steeply along its flow (`fall`, rise per
// metre). `sea`: the deep colour of the regions about the eye. `pixel` (metres of a pixel per metre of distance)
// and `curvature` are the frame's. Linear light, before the haze.
// #include ../ground/look.wgsl

// The body of the water in linear light.
const BODY: vec3f = vec3f(0.025, 0.098, 0.175);
// The water turns white where it falls more steeply than FOAM_LOW along its flow, up to FOAM_MOST at FOAM_HIGH.
const FOAM_LOW: f32 = 0.4;
const FOAM_HIGH: f32 = 1.0;
const FOAM_MOST: f32 = 0.35;

fn water_look(relative: vec3f, lod: vec2f, fall: f32, sea: bool, pixel: f32, curvature: f32) -> vec3f {
    let dir = normalize(relative);
    let world = relative.xy + look.place.xy;
    // Small waves tilt the surface by the slope of the noise; far away, where a pixel spans several, it lies calm.
    let calm = 1.0 - smoothstep(0.5, 4.0, pixel * length(relative));
    let q = world / 1.7;
    let tilt = vec2f(noise(q + vec2f(0.35, 0.0)) - noise(q - vec2f(0.35, 0.0)), noise(q + vec2f(0.0, 0.35)) - noise(q - vec2f(0.0, 0.35))) * 0.06 * calm;
    let up = normalize(vec3f(2.0 * curvature * relative.xy - tilt, 1.0));
    let mirrored = reflect(dir, up);
    // The reflection is held to 90 %: the far bank that it would show is darker than the sky.
    let fresnel = 0.02 + 0.98 * pow(clamp(1.0 - dot(-dir, up), 0.0, 1.0), 5.0);
    let sun = sun_share(sun_at(lod), relative, vec3f(0.0, 0.0, 1.0), LIFT_GROUND);
    let lit = look.ambient.rgb + 0.8 * look.sun_colour.rgb * max(look.sun.z, 0.0) * sun;
    let body = select(BODY, look.sea.rgb, sea) * lit;
    var c = mix(body * 0.55, sky(mirrored), min(0.96 * fresnel, 0.9));
    // The sun's path glints in sparks rather than a sheen.
    let glint = max(dot(mirrored, look.sun.xyz), 0.0);
    let sparks = smoothstep(0.8, 0.86, noise(q + 17.0)) * calm;
    c += look.sun_colour.rgb * (4.0 * pow(glint, 400.0) + 2.5 * sparks * pow(glint, 80.0)) * sun;
    let foam = FOAM_MOST * smoothstep(FOAM_LOW, FOAM_HIGH, fall) * (0.6 + 0.4 * noise(world / 0.8));
    return mix(c, vec3f(0.85) * lit, foam);
}
