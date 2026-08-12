const TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c;
  }
  return table;
})();

export function crc32(chunk: Uint8Array, running = 0): number {
  let c = ~running;
  for (let index = 0; index < chunk.length; index++) {
    c = TABLE[(c ^ chunk[index]) & 0xff] ^ (c >>> 8);
  }
  return ~c >>> 0;
}
