import * as THREE from 'three';
import type { Plant } from '../library';
import { domed, GOLDEN, hash, hex, lathe, merge, noise, paint, ramp, random, relight, sheet, smooth, soft, UP, v3, type Ramp } from '../shape';
import type { Season } from '../trees';

/** Retama twigs: grey olive, browner in winter. */
const RETAMA: Record<Season, Ramp> = {
  winter: ['#454a38', '#767c5c', '#a4a684'],
  spring: ['#434e3a', '#768662', '#a8b48a'],
  summer: ['#434c3a', '#748260', '#a4b086'],
  autumn: ['#434c3a', '#748260', '#a2ae86'],
};

/** Retama flowers in late spring, black pods in summer: colour and share of twig tips. */
const RETAMA_BLOOM: Record<Season, [string, number]> = {
  winter: ['#969a76', 0],
  spring: ['#f8f2ee', 0.85],
  summer: ['#4a4030', 0.25],
  autumn: ['#969a76', 0],
};

/**
 * Teide broom: an almost hemispherical bush of rush-thin upright twigs. A grey-olive shell carries
 * the mass; each twig is one sliver lying up it and standing out at its tip, so the twigs make
 * the outline. In flower the twig tips turn white, a few pink, and the shell pales a little. All
 * of it lights as the dome does. 270 triangles.
 */
function retama(season: Season, seed: number, rand: () => number): THREE.BufferGeometry {
  const [deep, mid, tip] = ramp(RETAMA[season]);
  const [flower, share] = RETAMA_BLOOM[season];
  const bloom = season === 'spring';
  const [petal, pink] = [hex(flower), hex('#f0c8d0')];
  const size = v3(0.95, 0.95, 1);
  const radius = (n: THREE.Vector3) => 1 + 0.05 * noise(n, 2.5, seed);
  // Flowers deep in the bush pale its shade in bloom, still grey-olive.
  const inside = deep.clone().lerp(mid, 0.25).lerp(hex('#c8c4a8'), bloom ? 0.3 : 0);
  const cards = sheet();
  const count = 210;
  for (let j = 0; j < count; j++) {
    const polar = Math.acos(1 - (j + 0.5) / count) * (0.97 + 0.04 * rand());
    const a = j * GOLDEN + rand() * 0.3;
    const n = v3(Math.sin(polar) * Math.cos(a), Math.sin(polar) * Math.sin(a), Math.cos(polar));
    const surface = n.clone().multiply(size).multiplyScalar(0.92 * radius(n));
    // Up the dome along its meridian but steeper, as broom twigs stand upright, so each lies on
    // the dome as a long streak and its tip stands out of it.
    const meridian = v3(-Math.cos(polar) * Math.cos(a), -Math.cos(polar) * Math.sin(a), Math.sin(polar));
    const dir = meridian.addScaledVector(UP, 0.8).addScaledVector(n, 0.25).normalize();
    const length = 0.32 + 0.12 * rand();
    const top = surface.clone().addScaledVector(dir, 0.45 * length);
    const foot = surface.clone().addScaledVector(dir, -0.55 * length).addScaledVector(n, -0.04);
    foot.z = Math.max(foot.z, 0);
    const twig = mid.clone().lerp(tip, 0.1 + 0.5 * rand());
    const flowers = hash(top, seed) < share;
    const end = flowers ? twig.clone().lerp(bloom && rand() < 0.18 ? pink : petal, bloom ? 0.95 : 0.5) : twig;
    // Flowering twigs are a little wider, as their flowers stand off them.
    const across = v3().crossVectors(dir, n);
    if (across.lengthSq() < 0.01) across.set(-Math.sin(a), Math.cos(a), 0);
    across.setLength((bloom && flowers ? 0.02 : 0.012) + 0.004 * rand());
    const lit = n.clone().lerp(UP, 0.15).normalize();
    cards.add([foot.clone().add(across), foot.clone().sub(across), top], [inside, twig.clone().lerp(inside, 0.5), end], () => lit);
  }
  const core = soft(lathe([[0.92, -0.03], [0.88, 0.42], [0.55, 0.8], [0, 0.9]], 12, 3));
  core.scale(0.9 * size.x, 0.9 * size.y, 0.9 * size.z);
  const shell = paint(relight(core, domed(v3(), size, 0.1)), (p) => inside.clone().lerp(mid, 0.3 * smooth(0.2, 0.8, p.z)));
  return merge([shell, cards.build()]);
}

export default {
  name: 'Retama',
  latin: 'Spartocytisus supranubius',
  build: (season, seed) => retama(season, seed, random(seed)),
  ground: { on: ['shrub', 'bare'], height: [1, 2], share: 0.1, own: true },
} satisfies Plant;
