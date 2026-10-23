// The kind registry: each module of kinds/ but this file and kind.ts draws the features of the kind of
// its name.
import type { Kind } from './kind';

const MODULES = import.meta.glob<Kind>(['./*.ts', '!./index.ts', '!./kind.ts', '!./*.test.ts'], { eager: true, import: 'default' });

export const KINDS = new Map(Object.entries(MODULES).map(([path, kind]) => [path.slice(2, -3), kind]));
