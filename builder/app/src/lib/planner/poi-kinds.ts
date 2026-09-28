export type PlaceCategory = 'hotel' | 'camp' | 'shop' | 'water' | 'bike' | 'station';

/** Basemap `pois` kinds the planner shows as places. */
export const poiKinds: Record<string, { category: PlaceCategory; label: string }> = {
    hotel: { category: 'hotel', label: 'Hotel' },
    hostel: { category: 'hotel', label: 'Hostel' },
    guest_house: { category: 'hotel', label: 'Guest house' },
    alpine_hut: { category: 'hotel', label: 'Alpine hut' },
    camp_site: { category: 'camp', label: 'Campsite' },
    shelter: { category: 'camp', label: 'Shelter' },
    supermarket: { category: 'shop', label: 'Supermarket' },
    convenience: { category: 'shop', label: 'Convenience shop' },
    bakery: { category: 'shop', label: 'Bakery' },
    drinking_water: { category: 'water', label: 'Drinking water' },
    fountain: { category: 'water', label: 'Fountain' },
    bicycle: { category: 'bike', label: 'Bike shop' },
    station: { category: 'station', label: 'Station' },
};
