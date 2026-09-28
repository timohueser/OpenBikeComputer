/** Rider place categories: `label` names one place, `plural` the layer, `icon` is a 24 px stroke path, `kinds` maps basemap `pois` kinds to names. */
export const placeCategories = {
    hotel: {
        label: 'Lodging', plural: 'Lodging',
        icon: 'M3 19V8m18 11v-5M3 15h18M3 11h7v4M13 15v-4h5a3 3 0 0 1 3 3',
        kinds: { hotel: 'Hotel', hostel: 'Hostel', guest_house: 'Guest house', motel: 'Motel', chalet: 'Chalet', alpine_hut: 'Alpine hut' },
    },
    camp: {
        label: 'Campsite', plural: 'Campsites',
        icon: 'M3 20 12 4l9 16H3Zm5 0 4-8 4 8',
        kinds: { camp_site: 'Campsite', caravan_site: 'Caravan site' },
    },
    water: {
        label: 'Drinking water', plural: 'Water',
        icon: 'M12 3s-6 7-6 11.5a6 6 0 0 0 12 0C18 10 12 3 12 3Z',
        kinds: { drinking_water: 'Drinking water', fountain: 'Fountain', spring: 'Spring' },
    },
    shop: {
        label: 'Food shop', plural: 'Food shops',
        icon: 'M3 5h2l2 10h11l2-7H6m3 12h.01M17 20h.01',
        kinds: { supermarket: 'Supermarket', convenience: 'Convenience shop', bakery: 'Bakery', greengrocer: 'Greengrocer', marketplace: 'Market' },
    },
    food: {
        label: 'Food', plural: 'Eating',
        icon: 'M6 3v6a2 2 0 0 0 4 0V3M8 11v10M17 21V3c-2 1-3 4-3 8h3',
        kinds: { restaurant: 'Restaurant', cafe: 'Café', fast_food: 'Fast food', pub: 'Pub', biergarten: 'Beer garden' },
    },
    bike: {
        label: 'Bike service', plural: 'Bike',
        icon: 'M2.5 16a3.5 3.5 0 1 0 7 0 3.5 3.5 0 1 0-7 0m12 0a3.5 3.5 0 1 0 7 0 3.5 3.5 0 1 0-7 0M6 16l4-7h6l2 7m-8-7 3 7h2M8 6h3',
        kinds: { bicycle: 'Bike shop', bicycle_repair_station: 'Repair station', bicycle_rental: 'Bike rental' },
    },
    pharmacy: {
        label: 'Pharmacy', plural: 'Pharmacy',
        icon: 'M9 4h6v5h5v6h-5v5H9v-5H4V9h5V4Z',
        kinds: { pharmacy: 'Pharmacy' },
    },
    station: {
        label: 'Station', plural: 'Stations',
        icon: 'M6 3h12v12H6V3Zm0 8h12M9 18l-2 3m8-3 2 3',
        kinds: { station: 'Station', train_station: 'Station', bus_station: 'Bus station' },
    },
    shelter: {
        label: 'Shelter', plural: 'Shelters',
        icon: 'M2 12 12 4l10 8M5 10v10m14-10v10',
        kinds: { shelter: 'Shelter' },
    },
    toilets: {
        label: 'Toilets', plural: 'Toilets',
        icon: 'M12 3v18M7 5h.01M17 5h.01M5 9h4v5H8v6H6v-6H5V9Zm10 0h4l1 7h-2v4h-2v-4h-2l1-7Z',
        kinds: { toilets: 'Toilets' },
    },
    viewpoint: {
        label: 'Viewpoint', plural: 'Viewpoints',
        icon: 'M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12Zm7 0a3 3 0 1 0 6 0 3 3 0 1 0-6 0',
        kinds: { viewpoint: 'Viewpoint' },
    },
    peak: {
        label: 'Peak', plural: 'Peaks and passes',
        icon: 'M3 20 12 5l9 15H3Zm5.5-6 3.5-2 3.5 2',
        kinds: { peak: 'Peak', saddle: 'Pass', mountain_pass: 'Pass' },
    },
};

export type PlaceCategory = keyof typeof placeCategories;
export const categoryIds = Object.keys(placeCategories) as PlaceCategory[];

/** Basemap `pois` kinds the planner shows as places. */
export const poiKinds: Record<string, { category: PlaceCategory; label: string }> = Object.fromEntries(
    categoryIds.flatMap(category => Object.entries(placeCategories[category].kinds).map(([kind, label]) => [kind, { category, label }])),
);
