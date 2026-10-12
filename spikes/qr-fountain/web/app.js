import { Encoder } from './fountain.js';
import { encodeRoute, gpxRoute, lengthKm, syntheticRoute } from './route.js';

const $ = (id) => document.getElementById(id);
const canvas = $('qr');
let gpxPoints = null;
let timer = null;

$('gpx').addEventListener('change', async (e) => {
  const file = e.target.files[0];
  gpxPoints = file ? gpxRoute(await file.text()) : null;
  if (timer) start();
});
for (const id of ['points', 'blockSize', 'fps', 'ecc']) $(id).addEventListener('change', () => timer && start());
$('toggle').addEventListener('click', () => (timer ? stop() : start()));

function stop() {
  clearInterval(timer);
  timer = null;
  $('toggle').textContent = 'Start';
}

async function start() {
  stop();
  const points = gpxPoints ?? syntheticRoute(+$('points').value);
  const { raw, payload } = await encodeRoute(points);
  const encoder = new Encoder(payload, +$('blockSize').value);
  const fps = +$('fps').value;
  const ecc = $('ecc').value;
  let seq = 0;
  const show = () => {
    const qr = qrcode(0, ecc);
    qr.addData(encoder.frame(seq), 'Alphanumeric');
    qr.make();
    draw(qr);
    stats({
      Points: points.length.toLocaleString(),
      Length: `${lengthKm(points).toFixed(0)} km`,
      'Raw bytes': raw.toLocaleString(),
      'Payload bytes': payload.length.toLocaleString(),
      Blocks: encoder.blockCount,
      'QR version': (qr.getModuleCount() - 17) / 4,
      'Best case': `${(encoder.blockCount / fps).toFixed(1)} s`,
      Frame: seq,
    });
    seq++;
  };
  show();
  timer = setInterval(show, 1000 / fps);
  $('toggle').textContent = 'Stop';
}

function draw(qr) {
  const n = qr.getModuleCount() + 8; // 4-module quiet zone on each side
  const room = Math.min(innerHeight, innerWidth - 300) * 0.92;
  const scale = Math.max(1, Math.floor(room / n));
  canvas.width = canvas.height = n * scale;
  const ctx = canvas.getContext('2d');
  ctx.fillStyle = '#fff';
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  ctx.fillStyle = '#000';
  for (let r = 0; r < n - 8; r++)
    for (let c = 0; c < n - 8; c++) if (qr.isDark(r, c)) ctx.fillRect((c + 4) * scale, (r + 4) * scale, scale, scale);
}

function stats(rows) {
  $('stats').innerHTML = Object.entries(rows).map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`).join('');
}
