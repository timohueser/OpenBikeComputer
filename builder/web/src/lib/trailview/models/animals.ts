// The animals of the herds: one file per breed in animals/, whose name is the breed's id, by which packs name it. A breed
// builds each of its coats in each of its poses with the modelling kit (sculpt.ts): 1 m tall at the withers' top when
// it stands, feet at the origin, z up, facing +y; the instance scales it to the breed's height.
import type { Sculpt } from './sculpt';

export interface Animal<Coat = unknown> {
    name: string;
    latin: string;
    /** Metres to the top of the back, which the instance scales to. */
    height: number;
    coats: Coat[];
    /** The count of poses; the herds draw each about as often. */
    poses: number;
    build(s: Sculpt, coat: Coat, pose: number): void;
}

export const ANIMALS = Object.fromEntries(Object.entries(import.meta.glob('./animals/*.ts', { eager: true, import: 'default' }))
    .map(([path, animal]) => [path.split('/').pop()!.replace(/\.ts$/, ''), animal as Animal]));
