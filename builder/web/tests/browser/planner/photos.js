/** Photo files for the browser tests: a picture that the page draws, with the EXIF block of a camera. */

/** The EXIF block of a photo that a camera took at `time`, on a clock in UTC with its zone. */
function exif(time) {
  const u16 = value => [value & 255, value >> 8], u32 = value => [...u16(value & 0xffff), ...u16(value >>> 16)];
  const ascii = text => [...text].map(letter => letter.charCodeAt(0));
  const taken = `${new Date(time * 1000).toISOString().slice(0, 19).replace(/-/g, ':').replace('T', ' ')}\0`;
  // The first directory, at 8, points to the EXIF directory at 26; the time and the zone follow it, at 56 and at 76.
  const tiff = [0x49, 0x49, 42, 0, ...u32(8), ...u16(1), ...u16(0x8769), ...u16(4), ...u32(1), ...u32(26), ...u32(0),
    ...u16(2), ...u16(0x9003), ...u16(2), ...u32(20), ...u32(56), ...u16(0x9011), ...u16(2), ...u32(7), ...u32(76), ...u32(0), ...ascii(taken), ...ascii('+00:00\0')];
  return Buffer.from([0xff, 0xe1, (tiff.length + 8) >> 8, (tiff.length + 8) & 255, ...ascii('Exif\0\0'), ...tiff]);
}

/** A JPEG file of a hill under a sky, drawn in the page, with no time. */
export async function drawnJpeg(page) {
  return Buffer.from(await page.evaluate(async () => {
    const canvas = Object.assign(document.createElement('canvas'), { width: 320, height: 240 }), context = canvas.getContext('2d');
    context.fillStyle = '#9cc3dc';
    context.fillRect(0, 0, 320, 240);
    context.fillStyle = '#4f7a3a';
    context.beginPath();
    context.ellipse(200, 260, 260, 130, 0, 0, Math.PI * 2);
    context.fill();
    const blob = await new Promise(done => canvas.toBlob(done, 'image/jpeg'));
    return [...new Uint8Array(await blob.arrayBuffer())];
  }));
}

/** A file for `setFiles`: the JPEG `jpeg` with the time `time`. */
export const photoFile = (jpeg, name, time) => ({ name, mimeType: 'image/jpeg', buffer: Buffer.concat([jpeg.subarray(0, 2), exif(time), jpeg.subarray(2)]) });
