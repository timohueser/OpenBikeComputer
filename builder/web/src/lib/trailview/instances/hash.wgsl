// The hashes of the producers and of what the ground paints for them: a place's values come from whole
// world cells, the same at every distance. Include it with `// #include ../instances/hash.wgsl`.

fn mix_bits(a: u32) -> u32 {
    var x = a * 747796405u + 2891336453u;
    x = ((x >> ((x >> 28u) + 4u)) ^ x) * 277803737u;
    return (x >> 22u) ^ x;
}

fn mix_bits3(x: i32, y: i32, z: u32) -> u32 {
    return mix_bits(u32(x) ^ mix_bits(u32(y) ^ mix_bits(z)));
}

fn unit_float(h: u32) -> f32 {
    return f32(h >> 8u) / 16777216.0;
}
