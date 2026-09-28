/* Example data, all from wireframes/kit/content.md. Coordinates are in the units of the named map
   (see MAPS). A place with `day` and `km` sits on the Alps line at that base day's km; when it has
   no x/y, Trip.init places it on the line. */

const MAPS = {
  // x = (lon − lon0) / dlon × w ; y = (lat0 − lat) / dlat × h ; kmx/kmy = km per map unit
  bf:   { w: 1000, h: 700, lon0: 7.75, dlon: 0.5,  lat0: 48.15, dlat: 0.3, kmx: 0.036, kmy: 0.047 },
  alps: { w: 700,  h: 900, lon0: 5.6,  dlon: 2.0,  lat0: 46.3,  dlat: 2.7, kmx: 0.224, kmy: 0.333 },
  d4:   { w: 1000, h: 600, lon0: 6.35, dlon: 0.75, lat0: 45.5,  dlat: 0.4, kmx: 0.059, kmy: 0.074 },
};

const KINDS = {
  campsite: { label: 'Campsites', one: 'campsite', icon: 'i-tent',   sleep: true },
  lodging:  { label: 'Lodging',   one: 'lodging',  icon: 'i-bed',    sleep: true },
  hut:      { label: 'Huts',      one: 'hut',      icon: 'i-tent',   sleep: true },
  shelter:  { label: 'Shelters',  one: 'shelter',  icon: 'i-tent',   sleep: true },
  shop:     { label: 'Shops',     one: 'shop',     icon: 'i-cart' },
  water:    { label: 'Water',     one: 'water point', icon: 'i-drop' },
  pharmacy: { label: 'Pharmacies', one: 'pharmacy', icon: 'i-cross' },
  bikeshop: { label: 'Bike shops', one: 'bike shop', icon: 'i-wrench' },
  train:    { label: 'Train stations', one: 'station', icon: 'i-train' },
};
const SLEEP = ['campsite', 'lodging', 'hut', 'shelter'];
const SUPPLY = ['shop', 'water', 'pharmacy', 'bikeshop', 'train'];

// Point types of a route, with their glyphs.
const POINT_KINDS = { pass: { label: 'Pass here', icon: 'i-ring' }, shape: { label: 'Shape', icon: 'i-dot' }, visit: { label: 'Visit', icon: 'i-flag' }, sleep: { label: 'Sleep', icon: 'i-tent' }, marker: { label: 'Marker', icon: 'i-drop' } };
const MODES = { routed: 'Routed', straight: 'Straight', drawn: 'Drawn' };

const BIKES = { road: 'Road', gravel: 'Gravel', mtb: 'MTB', touring: 'Touring' };
const GOALS = { balanced: 'Balanced', shortest: 'Shortest', leastclimb: 'Least climbing', leastunpaved: 'Least unpaved', mostclimb: 'Most climbing' };
const WEEKDAYS = { mon: 'Monday', tue: 'Tuesday', wed: 'Wednesday', thu: 'Thursday', fri: 'Friday', sat: 'Saturday', sun: 'Sunday' };

// The Alps trip's base days. Times in minutes. The days the rider sees derive from these (Trip).
const DAYS = [
  { n: 1, from: 'Genève', to: 'Le Grand-Bornand', km: 78, climb: 1640, time: 340 },
  { n: 2, from: 'Le Grand-Bornand', to: 'Beaufort', km: 64, climb: 1720, time: 320 },
  { n: 3, from: 'Beaufort', to: "Val d'Isère", km: 71, climb: 2290, time: 390 },
  { n: 4, from: "Val d'Isère", to: 'Valloire', km: 104, climb: 2310, time: 430 },
  { n: 5, from: 'Valloire', to: 'Briançon', km: 53, climb: 1480, time: 270 },
  { n: 6, rest: true, to: 'Briançon', km: 0, climb: 0, time: 0 },
  { n: 7, from: 'Briançon', to: 'Guillestre', km: 56, climb: 1420, time: 280 },
  { n: 8, from: 'Guillestre', to: 'Barcelonnette', km: 51, climb: 1470, time: 270 },
  { n: 9, from: 'Barcelonnette', to: 'Saint-Étienne-de-Tinée', km: 63, climb: 1640, time: 330 },
  { n: 10, from: 'Saint-Étienne-de-Tinée', to: 'Nice', km: 94, climb: 620, time: 290 },
];
const TRIP = { km: 634, climb: 14590, time: 2940, facts: '634 km · 14,590 m · 49 h riding', unknown: '12 km unknown surface', dates: 'Thu 17 – Sat 26 Jun 2027', datesShort: '17–26 Jun 2027', start: '2027-06-17' };
// Profile bands in trip km.
const BANDS = {
  unknown: [[104, 112], [398, 402]], unpaved: [[118, 119.5], [400, 401]],
  snow: [[222, 228], [497, 501], [331, 335]], closure: [[220, 229]],
};

// P(name, kind, map, x, y, more). Kinds: city town village summit pass lake lodging campsite shop water bikeshop
const P = (name, kind, map, x, y, more) => Object.assign({ id: name.toLowerCase().normalize('NFD').replace(/[̀-ͯ]/g, '').replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, ''), name, kind, map, x, y }, more);
const PLACES = [
  // Black Forest
  P('Freiburg im Breisgau', 'city', 'bf', 200, 362, { elev: 278, region: 'Black Forest' }),
  P('Denzlingen', 'town', 'bf', 256, 191, { elev: 235, region: 'Black Forest' }),
  P('Waldkirch', 'town', 'bf', 424, 131, { elev: 263, region: 'Black Forest' }),
  P('Kandel', 'summit', 'bf', 536, 203, { elev: 1241, region: 'Black Forest', alias: ['kandel summit', 'kandel mountain'], ambiguous: true }),
  P('Glottertal', 'village', 'bf', 380, 233, { elev: 310, region: 'Black Forest' }),
  P('St. Peter', 'village', 'bf', 566, 310, { elev: 722, region: 'Black Forest' }),
  P('St. Märgen', 'village', 'bf', 686, 369, { elev: 889, region: 'Black Forest' }),
  P('Kirchzarten', 'town', 'bf', 410, 434, { elev: 392, region: 'Black Forest' }),
  P('Hinterzarten', 'village', 'bf', 712, 576, { elev: 893, region: 'Black Forest' }),
  P('Titisee', 'village', 'bf', 820, 576, { elev: 846, region: 'Black Forest', sub: 'lake and village' }),
  P('Schauinsland', 'summit', 'bf', 296, 558, { elev: 1284, region: 'Black Forest' }),
  P('Feldberg', 'summit', 'bf', 508, 644, { elev: 1493, region: 'Black Forest' }),
  P('Simonswald', 'village', 'bf', 620, 117, { elev: 330, region: 'Black Forest' }),
  P('Hotel Krone, St. Peter', 'lodging', 'bf', 590, 318, { elev: 720, region: 'Black Forest', off: 1.1, alias: ['hotel krone'] }),
  // Alps overview
  P('Genève', 'city', 'alps', 190, 32, { region: 'Alps', alias: ['geneva', 'genf'], day: 1, km: 0, elev: 375 }),
  P('Le Grand-Bornand', 'village', 'alps', 290, 119, { region: 'Alps', day: 1, km: 78, elev: 950 }),
  P('Beaufort', 'village', 'alps', 341, 194, { region: 'Alps', day: 2, km: 64, elev: 740 }),
  P("Lac d'Annecy", 'lake', 'alps', 200, 150, { region: 'Alps', alias: ['annecy'] }),
  P('Col de la Colombière', 'pass', 'alps', null, null, { elev: 1613, region: 'Alps', day: 1, km: 65, alias: ['colombiere'] }),
  P('Col des Aravis', 'pass', 'alps', null, null, { elev: 1486, region: 'Alps', day: 2, km: 12, alias: ['aravis'] }),
  P('Col des Saisies', 'pass', 'alps', null, null, { elev: 1650, region: 'Alps', day: 2, km: 42, alias: ['saisies'] }),
  P('Cormet de Roselend', 'pass', 'alps', null, null, { elev: 1968, region: 'Alps', day: 3, km: 23, alias: ['roselend'] }),
  P('Col du Galibier', 'pass', 'alps', 283, 412, { elev: 2642, region: 'Alps', day: 5, km: 18, alias: ['galibier'] }),
  P('Briançon', 'town', 'alps', 363, 467, { region: 'Alps', alias: ['briancon'], day: 5, km: 53, elev: 1200 }),
  P("Col d'Izoard", 'pass', 'alps', 397, 493, { elev: 2360, region: 'Alps', day: 7, km: 20, alias: ['izoard'] }),
  P('Guillestre', 'village', 'alps', 367, 547, { region: 'Alps', day: 7, km: 56, elev: 1000 }),
  P('Col de Vars', 'pass', 'alps', 386, 587, { elev: 2109, region: 'Alps', day: 8, km: 20, alias: ['vars'] }),
  P('Barcelonnette', 'town', 'alps', 368, 638, { region: 'Alps', day: 8, km: 51, elev: 1130 }),
  P('Cime de la Bonette', 'pass', 'alps', 422, 660, { elev: 2802, region: 'Alps', day: 9, km: 24, alias: ['bonette'] }),
  P('Saint-Étienne-de-Tinée', 'village', 'alps', 464, 681, { region: 'Alps', day: 9, km: 63, elev: 1140 }),
  P('Nice', 'city', 'alps', 583, 866, { region: 'Alps', day: 10, km: 94, elev: 10 }),
  // Day 4 map, with km on Day 4
  P("Val d'Isère", 'village', 'd4', 840, 78, { elev: 1850, region: 'Alps', day: 4, km: 0 }),
  P("Col de l'Iseran", 'pass', 'd4', 908, 125, { elev: 2764, region: 'Alps', day: 4, km: 15, alias: ['iseran'] }),
  P('Bonneval-sur-Arc', 'village', 'd4', 929, 192, { elev: 1780, region: 'Alps', day: 4, km: 30 }),
  P('Bessans', 'village', 'd4', 853, 270, { elev: 1710, region: 'Alps', day: 4, km: 38 }),
  P('Lanslevillard', 'village', 'd4', 747, 310, { elev: 1450, region: 'Alps', day: 4, km: 45 }),
  P('Lanslebourg-Mont-Cenis', 'village', 'd4', 704, 321, { elev: 1400, region: 'Alps', day: 4, km: 48, alias: ['lanslebourg'] }),
  P('Termignon', 'village', 'd4', 620, 334, { elev: 1300, region: 'Alps', day: 4, km: 54 }),
  P('Bramans', 'village', 'd4', 567, 412, { elev: 1230, region: 'Alps', day: 4, km: 62 }),
  P('Modane', 'town', 'd4', 427, 450, { elev: 1060, region: 'Alps', day: 4, km: 71 }),
  P('Saint-Michel-de-Maurienne', 'town', 'd4', 160, 420, { elev: 710, region: 'Alps', day: 4, km: 88, alias: ['saint michel', 'st michel'] }),
  P('Col du Télégraphe', 'pass', 'd4', 125, 445, { elev: 1566, region: 'Alps', day: 4, km: 99, alias: ['telegraphe'] }),
  P('Valloire', 'village', 'd4', 105, 501, { elev: 1430, region: 'Alps', day: 4, km: 104 }),
  // Places on Day 4 (the tent job and the box results)
  P('Camping Caravaneige, Valloire', 'campsite', 'd4', 140, 522, { day: 4, km: 104, off: 0.6, climb: 30, hours: 'Reception 8:00–20:00' }),
  P('Camping Les Verneys, Valloire', 'campsite', 'd4', 84, 530, { day: 4, km: 103, off: 1.4, climb: 70, hours: 'Hours unknown' }),
  P('Hôtel Christiania, Valloire', 'lodging', 'd4', 122, 486, { day: 4, km: 104, off: 0.2, climb: 0 }),
  P('Camping Le Petit Nice, Saint-Michel-de-Maurienne', 'campsite', 'd4', 172, 406, { day: 4, km: 87, off: 0.9, climb: 10, hours: 'Hours unknown' }),
  P("Camping de l'Arc, Modane", 'campsite', 'd4', 434, 458, { day: 4, km: 71, off: 0.4, climb: 0, hours: 'Reception 9:00–19:00' }),
  P('Camping Les Mélèzes, Lanslevillard', 'campsite', 'd4', 752, 318, { day: 4, km: 45, off: 0.3, climb: 10, hours: 'Hours unknown' }),
  P('Sherpa, Valloire', 'shop', 'd4', 92, 494, { day: 4, km: 104, off: 0.3, climb: 0, hours: 'Sun 8:00–12:30, 16:00–19:30', sun: true }),
  P('Carrefour Market, Modane', 'shop', 'd4', 420, 462, { day: 4, km: 71, off: 0.5, climb: 0, hours: 'Sun 8:30–12:30', sun: true }),
  P('Intermarché, Saint-Michel-de-Maurienne', 'shop', 'd4', 150, 432, { day: 4, km: 88, off: 0.7, climb: 0, hours: 'Sun closed', sun: false }),
  P('Épicerie de Bonneval', 'shop', 'd4', 940, 200, { day: 4, km: 30, off: 0.1, climb: 0, hours: 'Hours unknown' }),
  P('Vival, Termignon', 'shop', 'd4', 630, 344, { day: 4, km: 54, off: 0.2, climb: 0, hours: 'Sun 8:00–12:00', sun: true }),
  P('Fountain, Bonneval-sur-Arc', 'water', 'd4', 918, 198, { day: 4, km: 30, off: 0.0, climb: 0 }),
  P('Fountain, Lanslebourg', 'water', 'd4', 712, 330, { day: 4, km: 48, off: 0.1, climb: 0 }),
  P('Cycles Maurienne, Saint-Michel-de-Maurienne', 'bikeshop', 'd4', 168, 432, { day: 4, km: 88, off: 0.5, climb: 0, hours: 'Hours unknown' }),
];
// Not on any map: the other Kandel.
const OTHER_KANDEL = { name: 'Kandel', line: 'Town · Rhineland-Palatinate · 138 km' };

const KIND_LINE = { city: 'City', town: 'Town', village: 'Village', summit: 'Mountain summit', pass: 'Pass', lake: 'Lake', lodging: 'Lodging', campsite: 'Campsite', shop: 'Shop', water: 'Water', bikeshop: 'Bike shop' };

const ROUTES = {
  titisee: {
    from: 'glottertal', to: 'titisee', title: 'Glottertal → Titisee',
    options: [
      { id: 'shortest', name: 'Shortest', win: '34 km', meta: '1,020 m · 2 h 50 · 3.1 km unpaved', km: 34, climb: 1020, time: 170, path: 'route-bf-shortest', prof: 'prof-bf-titisee', label: ['Shortest · 34 km', 470, 262], surface: { unpaved: [[12.5, 15.6]], unknown: [[14.2, 15.6]] } },
      { id: 'leastclimb', name: 'Least climbing', win: '−300 m', meta: '+13 km · +0 h 20 · via Freiburg, Höllental', km: 47, climb: 720, time: 190, path: 'route-bf-leastclimb', label: ['Least climbing · +13 km', 330, 500] },
      { id: 'leastunpaved', name: 'Least unpaved', win: '0 km unpaved', meta: '+2 km · +40 m · +0 h 10 · on roads', km: 36, climb: 1060, time: 180, path: 'route-bf-leastunpaved', label: ['Least unpaved · +2 km', 740, 440] },
    ],
  },
  kandel: {
    from: 'denzlingen', to: 'kandel', title: 'Denzlingen → Kandel summit', bike: 'road',
    options: [{ id: 'kandel', name: 'Road', km: 20.8, climb: 1020, time: 115, path: 'route-bf-kandel', prof: 'prof-bf-kandel',
      ledger: [['Distance', '20.8 km'], ['Climb', '1,020 m'], ['Riding time', '1 h 55 <small>estimated</small>'], ['Max gradient', '12 %'], ['Surface', 'Paved <small>0 km unknown</small>']],
      note: 'No other way is shorter, flatter or more paved.' }],
  },
  import: {
    title: 'Schwarzwald Gravel, 3 days', km: 312, climb: 6480, path: 'route-bf-import', prof: 'prof-bf-import',
    facts: '312 km · 6,480 m · Gravel', notes: ['Imported line · not re-routed', '41 km unknown surface', 'No mapped water for 38 km (Day 2)'],
    krone: { place: 'hotel-krone-st-peter', path: 'route-bf-krone', km: 2.3, climb: 60 },
  },
};

const GAPS = {
  water: { label: 'Longest stretches without mapped water', icon: 'i-drop',
    items: [{ title: 'No mapped water for 29 km', where: "Day 4 · km 0–29 · Col de l'Iseran", marker: "Marker at Val d'Isère", start: 'val-d-isere', band: [213, 242] }] },
  shop: { label: 'Longest stretches without a shop', icon: 'i-cart',
    items: [{ title: 'Last shop for 63 km', where: 'Day 9 · km 0–63 · Barcelonnette → Saint-Étienne-de-Tinée', marker: 'Marker at Barcelonnette', start: 'barcelonnette', band: [477, 540] }] },
};

const PLANS = [
  { id: 'alps', name: 'Alps: Genève to Nice', kind: 'trip', map: 'alps', bike: 'touring', goal: 'balanced', dates: true, today: 3 },
  { id: 'new', name: 'New route from Glottertal', kind: 'new', map: 'bf', here: 'glottertal', bike: 'touring', goal: 'balanced' },
  { id: 'import', name: 'Schwarzwald Gravel, 3 days', kind: 'import', map: 'bf', bike: 'gravel', goal: 'balanced', route: 'import' },
];

const DATA = { MAPS, KINDS, SLEEP, SUPPLY, POINT_KINDS, MODES, BIKES, GOALS, WEEKDAYS, DAYS, TRIP, BANDS, PLACES, OTHER_KANDEL, KIND_LINE, ROUTES, GAPS, PLANS,
  place: (id) => PLACES.find((p) => p.id === id) };
