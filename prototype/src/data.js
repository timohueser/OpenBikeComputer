/* Example data, all from wireframes/kit/content.md. Coordinates are in the units of the named map
   (see MAPS). Places on the Day 4 map carry d4 coordinates; the alps position is derived. */

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

const BIKES = { road: 'Road', gravel: 'Gravel', mtb: 'MTB', touring: 'Touring' };
const GOALS = { balanced: 'Balanced', shortest: 'Shortest', leastclimb: 'Least climbing', leastunpaved: 'Least unpaved', mostclimb: 'Most climbing' };
const WEEKDAYS = { mon: 'Monday', tue: 'Tuesday', wed: 'Wednesday', thu: 'Thursday', fri: 'Friday', sat: 'Saturday', sun: 'Sunday' };

// The Alps trip. Times in minutes; start = trip km at the day's start.
const DAYS = [
  { n: 1, wd: 'thu', date: 'Thu 17 Jun', from: 'Genève', to: 'Le Grand-Bornand', km: 78, climb: 1640, time: 340, passes: 'Col de la Colombière 1,613 m' },
  { n: 2, wd: 'fri', date: 'Fri 18 Jun', from: 'Le Grand-Bornand', to: 'Beaufort', km: 64, climb: 1720, time: 320, passes: 'Col des Aravis 1,486 m, Col des Saisies 1,650 m' },
  { n: 3, wd: 'sat', date: 'Sat 19 Jun', from: 'Beaufort', to: "Val d'Isère", km: 71, climb: 2290, time: 390, passes: 'Cormet de Roselend 1,968 m' },
  { n: 4, wd: 'sun', date: 'Sun 20 Jun', from: "Val d'Isère", to: 'Valloire', km: 104, climb: 2310, time: 430, passes: "Col de l'Iseran 2,764 m, Col du Télégraphe 1,566 m" },
  { n: 5, wd: 'mon', date: 'Mon 21 Jun', from: 'Valloire', to: 'Briançon', km: 53, climb: 1480, time: 270, passes: 'Col du Galibier 2,642 m' },
  { n: 6, wd: 'tue', date: 'Tue 22 Jun', rest: true, to: 'Briançon', km: 0, climb: 0, time: 0 },
  { n: 7, wd: 'wed', date: 'Wed 23 Jun', from: 'Briançon', to: 'Guillestre', km: 56, climb: 1420, time: 280, passes: "Col d'Izoard 2,360 m" },
  { n: 8, wd: 'thu', date: 'Thu 24 Jun', from: 'Guillestre', to: 'Barcelonnette', km: 51, climb: 1470, time: 270, passes: 'Col de Vars 2,109 m' },
  { n: 9, wd: 'fri', date: 'Fri 25 Jun', from: 'Barcelonnette', to: 'Saint-Étienne-de-Tinée', km: 63, climb: 1640, time: 330, passes: 'Cime de la Bonette 2,802 m' },
  { n: 10, wd: 'sat', date: 'Sat 26 Jun', from: 'Saint-Étienne-de-Tinée', to: 'Nice', km: 94, climb: 620, time: 290 },
];
const TRIP = { km: 634, climb: 14590, time: 2940, facts: '634 km · 14,590 m · 49 h riding', unknown: '12 km unknown surface', dates: 'Thu 17 – Sat 26 Jun 2027', datesShort: '17–26 Jun 2027' };
// Alps day ends on the overview map (Day 4's end is derived from its km, see D4_LINE).
const DAY_ENDS_ALPS = { 0: [190, 32], 1: [290, 119], 2: [341, 194], 3: [483, 284], 4: [290, 378], 5: [363, 467], 6: [363, 467], 7: [367, 547], 8: [368, 638], 9: [464, 681], 10: [583, 866] };
// Day 4 line: km → d4 position, from the content sheet's Day 4 map table.
const D4_LINE = [[0, 840, 78], [15, 908, 125], [30, 929, 192], [38, 853, 270], [45, 747, 310], [48, 704, 321], [54, 620, 334], [62, 567, 412], [71, 427, 450], [88, 160, 420], [99, 125, 445], [104, 105, 501]];
// Profile bands in trip km.
const BANDS = {
  unknown: [[104, 112], [398, 402]], unpaved: [[118, 119.5], [400, 401]],
  snow: [[222, 228], [497, 501], [331, 335]], closure: [[220, 229]],
};

// P(name, kind, map, x, y, more). Kinds: city town village summit pass lake lodging campsite shop water bikeshop
const P = (name, kind, map, x, y, more) => Object.assign({ id: name.toLowerCase().normalize('NFD').replace(/[\u0300-\u036f]/g, '').replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, ''), name, kind, map, x, y }, more);
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
  P('Genève', 'city', 'alps', 190, 32, { region: 'Alps', alias: ['geneva', 'genf'] }),
  P('Le Grand-Bornand', 'village', 'alps', 290, 119, { region: 'Alps' }),
  P('Beaufort', 'village', 'alps', 341, 194, { region: 'Alps' }),
  P("Lac d'Annecy", 'lake', 'alps', 200, 150, { region: 'Alps', alias: ['annecy'] }),
  P('Col du Galibier', 'pass', 'alps', 283, 412, { elev: 2642, region: 'Alps', day: 5, km: 18 }),
  P('Briançon', 'town', 'alps', 363, 467, { region: 'Alps', alias: ['briancon'] }),
  P("Col d'Izoard", 'pass', 'alps', 397, 493, { elev: 2360, region: 'Alps', day: 7 }),
  P('Guillestre', 'village', 'alps', 367, 547, { region: 'Alps' }),
  P('Col de Vars', 'pass', 'alps', 386, 587, { elev: 2109, region: 'Alps', day: 8 }),
  P('Barcelonnette', 'town', 'alps', 368, 638, { region: 'Alps' }),
  P('Cime de la Bonette', 'pass', 'alps', 422, 660, { elev: 2802, region: 'Alps', day: 9 }),
  P('Saint-Étienne-de-Tinée', 'village', 'alps', 464, 681, { region: 'Alps' }),
  P('Nice', 'city', 'alps', 583, 866, { region: 'Alps' }),
  // Day 4 map, with km on Day 4
  P("Val d'Isère", 'village', 'd4', 840, 78, { elev: 1850, region: 'Alps', day: 4, km: 0 }),
  P("Col de l'Iseran", 'pass', 'd4', 908, 125, { elev: 2764, region: 'Alps', day: 4, km: 15 }),
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
      { id: 'shortest', name: 'Shortest', win: '34 km', meta: '1,020 m · 2 h 50 · 3.1 km unpaved', km: 34, climb: 1020, time: 170, path: 'route-bf-shortest', prof: 'prof-bf-titisee', profKm: 34, label: ['Shortest · 34 km', 470, 262], surface: { unpaved: [[12.5, 15.6]], unknown: [[14.2, 15.6]] } },
      { id: 'leastclimb', name: 'Least climbing', win: '−300 m', meta: '+13 km · +0 h 20 · via Freiburg, Höllental', km: 47, climb: 720, time: 190, path: 'route-bf-leastclimb', label: ['Least climbing · +13 km', 330, 500] },
      { id: 'leastunpaved', name: 'Least unpaved', win: '0 km unpaved', meta: '+2 km · +40 m · +0 h 10 · on roads', km: 36, climb: 1060, time: 180, path: 'route-bf-leastunpaved', label: ['Least unpaved · +2 km', 740, 440] },
    ],
  },
  kandel: {
    from: 'denzlingen', to: 'kandel', title: 'Denzlingen → Kandel summit', bike: 'road',
    options: [{ id: 'kandel', name: 'Road', km: 20.8, climb: 1020, time: 115, path: 'route-bf-kandel', prof: 'prof-bf-kandel', profKm: 20.8,
      ledger: [['Distance', '20.8 km'], ['Climb', '1,020 m'], ['Riding time', '1 h 55 <small>estimated</small>'], ['Max gradient', '12 %'], ['Surface', 'Paved <small>0 km unknown</small>']],
      note: 'No other way is shorter, flatter or more paved.' }],
  },
  import: {
    title: 'Schwarzwald Gravel, 3 days', km: 312, climb: 6480, path: 'route-bf-import', prof: 'prof-bf-import', profKm: 312,
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

const DATA = { MAPS, KINDS, SLEEP, SUPPLY, BIKES, GOALS, WEEKDAYS, DAYS, TRIP, DAY_ENDS_ALPS, D4_LINE, BANDS, PLACES, OTHER_KANDEL, KIND_LINE, ROUTES, GAPS, PLANS,
  place: (id) => PLACES.find((p) => p.id === id) };
