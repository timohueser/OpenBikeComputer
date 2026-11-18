// PROTOTYPE (prototype/save-bar): fake state of the new Save, set from `?proto=<state>&alt=B` in the URL.
const params = new URLSearchParams(location.search);
const state = params.get('proto') ?? 'unsaved';

export type FakeVersion = { at: string; summary: string };

const threeVersions: FakeVersion[] = [
    { at: 'Today 14:05', summary: '84.2 km · 1,240 m' },
    { at: 'Yesterday 18:40', summary: '79.6 km · 1,110 m' },
    { at: '12 Nov 09:12', summary: '91.0 km · 1,385 m' },
];

export const proto = $state({
    alt: (params.get('alt') ?? 'A') as 'A' | 'B' | 'C',
    stored: !['unsaved', 'name'].includes(state),
    changed: ['unsaved', 'name', 'changed', 'menu', 'menu0', 'switch'].includes(state),
    name: params.get('name') ?? '',
    versions: state === 'menu0' || state === 'unsaved' || state === 'name' ? [] : threeVersions,
    window: (state === 'switch' ? 'switch' : null) as null | 'name' | 'switch',
    reminder: state === 'reminder' ? 'Yesterday 18:40' : '',
});

(window as unknown as { proto: typeof proto }).proto = proto;
