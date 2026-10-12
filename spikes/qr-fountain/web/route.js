// Route payload: deflate-raw( varint count | per point zigzag varint dLat, dLon (µdeg), dEle (dm) ).
// ios/QRFountain/Route.swift decodes it.

export function syntheticRoute(count) {
  const points = [];
  let lat = 46.56, lon = 8.33, ele = 1800, heading = Math.random() * 2 * Math.PI;
  for (let i = 0; i < count; i++) {
    heading += (Math.random() - 0.5) * 0.6;
    const step = 5 + Math.random() * 25; // metres
    lat += (step * Math.cos(heading)) / 111_320;
    lon += (step * Math.sin(heading)) / (111_320 * Math.cos((lat * Math.PI) / 180));
    ele = Math.max(0, ele + (Math.random() - 0.5) * 3);
    points.push({ lat, lon, ele });
  }
  return points;
}

export function gpxRoute(text) {
  const doc = new DOMParser().parseFromString(text, 'application/xml');
  return [...doc.querySelectorAll('trkpt, rtept')].map((p) => ({
    lat: +p.getAttribute('lat'),
    lon: +p.getAttribute('lon'),
    ele: +(p.querySelector('ele')?.textContent ?? 0),
  }));
}

export function lengthKm(points) {
  let m = 0;
  for (let i = 1; i < points.length; i++) {
    const a = points[i - 1], b = points[i];
    const dy = (b.lat - a.lat) * 111_320;
    const dx = (b.lon - a.lon) * 111_320 * Math.cos((a.lat * Math.PI) / 180);
    m += Math.hypot(dx, dy);
  }
  return m / 1000;
}

export async function encodeRoute(points) {
  const bytes = [];
  const varint = (v) => {
    for (v >>>= 0; v >= 0x80; v >>>= 7) bytes.push((v & 0x7f) | 0x80);
    bytes.push(v);
  };
  const zigzag = (v) => (v << 1) ^ (v >> 31);
  varint(points.length);
  let pLat = 0, pLon = 0, pEle = 0;
  for (const p of points) {
    const lat = Math.round(p.lat * 1e6), lon = Math.round(p.lon * 1e6), ele = Math.round(p.ele * 10);
    varint(zigzag(lat - pLat));
    varint(zigzag(lon - pLon));
    varint(zigzag(ele - pEle));
    [pLat, pLon, pEle] = [lat, lon, ele];
  }
  const stream = new Blob([new Uint8Array(bytes)]).stream().pipeThrough(new CompressionStream('deflate-raw'));
  return { raw: bytes.length, payload: new Uint8Array(await new Response(stream).arrayBuffer()) };
}
