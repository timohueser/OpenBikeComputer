import { inflateRaw } from "pako";
import { crc32 } from "./crc32";

const HEADER_LEN = 128;
const ENTRY_LEN = 12;
const PAGE_CRC_LEN = 4;
const INTENSITY_NODATA = 15;

export interface ObcgHeader {
  totalLen: number;
  flags: number;
  validAt: number;
  referenceTime: number;
  southLatUdeg: number;
  westLonUdeg: number;
  cellLatUdeg: number;
  cellLonUdeg: number;
  width: number;
  height: number;
  cellSizeM: number;
  tileEdge: number;
  entriesPerPage: number;
  dataOffset: number;
  dataLen: number;
  objectCrc32: number;
  headerCrc32: number;
  tileCols: number;
  tileRows: number;
  tileCount: number;
  pageBytes: number;
  pageCount: number;
}

export interface TileEntry {
  dataOffset: number;
  encodedLen: number;
  codec: number;
  crc32: number;
}

export interface ObcgObject {
  bytes: Uint8Array;
  header: ObcgHeader;
  entries: TileEntry[];
}

const u16 = (view: DataView, offset: number) => view.getUint16(offset, true);
const u32 = (view: DataView, offset: number) => view.getUint32(offset, true);
const i32 = (view: DataView, offset: number) => view.getInt32(offset, true);
const i64 = (view: DataView, offset: number) => Number(view.getBigInt64(offset, true));

function fail(message: string): never {
  throw new Error(`Invalid OBCG object: ${message}`);
}

function crcWithZeroRange(bytes: Uint8Array, start: number, length: number): number {
  let crc = crc32(bytes.subarray(0, start));
  crc = crc32(new Uint8Array(length), crc);
  return crc32(bytes.subarray(start + length), crc);
}

export function parseObcg(buffer: ArrayBuffer, expectedObjectCrc?: number): ObcgObject {
  const bytes = new Uint8Array(buffer);
  if (bytes.length < HEADER_LEN) fail("short header");
  if (new TextDecoder().decode(bytes.subarray(0, 4)) !== "OBCG") fail("bad magic");
  const view = new DataView(buffer);
  if (u16(view, 4) !== 1 || u16(view, 6) !== HEADER_LEN) fail("unsupported version or header size");
  if (u16(view, 12) !== 0 || u16(view, 62) !== 0 || bytes.subarray(84, 128).some(Boolean)) fail("reserved bytes");

  const tileEdge = u16(view, 58);
  const entriesPerPage = u16(view, 60);
  const width = u32(view, 48);
  const height = u32(view, 52);
  if (tileEdge < 16 || tileEdge > 256 || (tileEdge & (tileEdge - 1)) !== 0) fail("tile edge");
  if (!entriesPerPage || entriesPerPage > 1365 || !width || !height) fail("grid geometry");
  const tileCols = Math.ceil(width / tileEdge);
  const tileRows = Math.ceil(height / tileEdge);
  const tileCount = tileCols * tileRows;
  const pageBytes = entriesPerPage * ENTRY_LEN + PAGE_CRC_LEN;
  const pageCount = Math.ceil(tileCount / entriesPerPage);
  const dataOffset = u32(view, 68);
  const dataLen = u32(view, 72);
  const totalLen = u32(view, 8);
  if (u32(view, 64) !== HEADER_LEN || dataOffset !== HEADER_LEN + pageCount * pageBytes) fail("section offsets");
  if (totalLen !== bytes.length || totalLen !== dataOffset + dataLen) fail("total length");

  const flags = u16(view, 14);
  if (flags !== 1 && flags !== 2) fail("source flags");
  const objectCrc32 = u32(view, 76);
  const headerCrc32 = u32(view, 80);
  if (expectedObjectCrc !== undefined && objectCrc32 !== expectedObjectCrc) fail("manifest CRC disagrees with header");
  if (crcWithZeroRange(bytes.subarray(0, HEADER_LEN), 80, 4) !== headerCrc32) fail("header CRC");
  if (crcWithZeroRange(bytes, 76, 8) !== objectCrc32) fail("object CRC");

  const header: ObcgHeader = {
    totalLen,
    flags,
    validAt: i64(view, 16),
    referenceTime: i64(view, 24),
    southLatUdeg: i32(view, 32),
    westLonUdeg: i32(view, 36),
    cellLatUdeg: u32(view, 40),
    cellLonUdeg: u32(view, 44),
    width,
    height,
    cellSizeM: u16(view, 56),
    tileEdge,
    entriesPerPage,
    dataOffset,
    dataLen,
    objectCrc32,
    headerCrc32,
    tileCols,
    tileRows,
    tileCount,
    pageBytes,
    pageCount,
  };

  const entries: TileEntry[] = [];
  let packedCursor = dataOffset;
  for (let page = 0; page < pageCount; page++) {
    const pageStart = HEADER_LEN + page * pageBytes;
    const pageEnd = pageStart + pageBytes;
    const storedPageCrc = view.getUint32(pageEnd - PAGE_CRC_LEN, true);
    if (crc32(bytes.subarray(pageStart, pageEnd - PAGE_CRC_LEN)) !== storedPageCrc) fail("directory page CRC");
    for (let slot = 0; slot < entriesPerPage; slot++) {
      const tileIndex = page * entriesPerPage + slot;
      const offset = pageStart + slot * ENTRY_LEN;
      if (tileIndex >= tileCount) {
        if (bytes.subarray(offset, offset + ENTRY_LEN).some(Boolean)) fail("directory padding");
        continue;
      }
      const entry = {
        dataOffset: u32(view, offset),
        encodedLen: u16(view, offset + 4),
        codec: bytes[offset + 6],
        crc32: u32(view, offset + 8),
      };
      if (bytes[offset + 7] !== 0) fail("directory reserved byte");
      if (entry.encodedLen === 0) {
        if (entry.dataOffset || entry.codec || entry.crc32) fail("dry sentinel");
      } else {
        if (entry.codec > 2 || entry.dataOffset !== packedCursor) fail("tile packing");
        packedCursor += entry.encodedLen;
        if (packedCursor > bytes.length) fail("tile bounds");
      }
      entries.push(entry);
    }
  }
  if (packedCursor !== bytes.length) fail("trailing or missing tile data");
  return { bytes, header, entries };
}

export function decodeTile(object: ObcgObject, tileIndex: number): Uint8Array | null {
  const entry = object.entries[tileIndex];
  if (!entry) fail("tile index");
  if (entry.encodedLen === 0) return null;
  const payload = object.bytes.subarray(entry.dataOffset, entry.dataOffset + entry.encodedLen);
  if (crc32(payload) !== entry.crc32) fail("tile CRC");
  const cellCount = object.header.tileEdge ** 2;
  let packed: Uint8Array;
  if (entry.codec === 2) {
    try {
      packed = inflateRaw(payload);
    } catch {
      return fail("deflate stream");
    }
    if (packed.length !== cellCount / 2) fail("deflate output length");
  } else if (entry.codec === 0) {
    if (payload.length !== cellCount / 2) fail("raw4 length");
    packed = payload;
  } else {
    const cells = new Uint8Array(cellCount);
    let cursor = 0;
    let previous = -1;
    let previousRun = 0;
    for (const byte of payload) {
      const value = byte & 0x0f;
      const run = (byte >>> 4) + 1;
      if ((value > 12 && value !== INTENSITY_NODATA) || (value === previous && previousRun !== 16)) fail("RLE4 data");
      if (cursor + run > cellCount) fail("RLE4 overrun");
      cells.fill(value, cursor, cursor + run);
      cursor += run;
      previous = value;
      previousRun = run;
    }
    if (cursor !== cellCount) fail("RLE4 underrun");
    return cells;
  }
  const cells = new Uint8Array(cellCount);
  for (let index = 0; index < packed.length; index++) {
    const byte = packed[index];
    const low = byte & 0x0f;
    const high = byte >>> 4;
    if ((low > 12 && low !== INTENSITY_NODATA) || (high > 12 && high !== INTENSITY_NODATA)) fail("reserved intensity");
    cells[index * 2] = low;
    cells[index * 2 + 1] = high;
  }
  return cells;
}

/**
 * Sample one south-up OBCG tile into a north-up screen raster. Low map zooms need only a handful
 * of pixels from each 256×256 tile, so expanding every nibble into 65,536 cells would turn a
 * world view into hundreds of megabytes of throwaway work.
 */
export function sampleTile(object: ObcgObject, tileIndex: number, width: number, height: number): Uint8Array | null {
  const entry = object.entries[tileIndex];
  if (!entry) fail("tile index");
  if (entry.encodedLen === 0) return null;
  const payload = object.bytes.subarray(entry.dataOffset, entry.dataOffset + entry.encodedLen);
  if (crc32(payload) !== entry.crc32) fail("tile CRC");
  const edge = object.header.tileEdge;
  const cellCount = edge * edge;
  const output = new Uint8Array(width * height);

  let packed: Uint8Array | null = null;
  if (entry.codec === 2) {
    try {
      packed = inflateRaw(payload);
    } catch {
      return fail("deflate stream");
    }
    if (packed.length !== cellCount / 2) fail("deflate output length");
  } else if (entry.codec === 0) {
    if (payload.length !== cellCount / 2) fail("raw4 length");
    packed = payload;
  }

  if (packed) {
    for (let y = 0; y < height; y++) {
      const sourceRow = edge - 1 - Math.min(edge - 1, Math.floor((y + 0.5) * edge / height));
      for (let x = 0; x < width; x++) {
        const sourceCol = Math.min(edge - 1, Math.floor((x + 0.5) * edge / width));
        const cellIndex = sourceRow * edge + sourceCol;
        const byte = packed[cellIndex >>> 1];
        const intensity = cellIndex & 1 ? byte >>> 4 : byte & 0x0f;
        if (intensity > 12 && intensity !== INTENSITY_NODATA) fail("reserved intensity");
        output[y * width + x] = intensity;
      }
    }
    return output;
  }

  // Validate RLE once, then walk samples in ascending source-cell order (bottom screen row first)
  // so each run is visited only once regardless of raster size.
  let total = 0;
  let previous = -1;
  let previousRun = 0;
  for (const byte of payload) {
    const value = byte & 0x0f;
    const run = (byte >>> 4) + 1;
    if ((value > 12 && value !== INTENSITY_NODATA) || (value === previous && previousRun !== 16)) fail("RLE4 data");
    total += run;
    if (total > cellCount) fail("RLE4 overrun");
    previous = value;
    previousRun = run;
  }
  if (total !== cellCount) fail("RLE4 underrun");
  let runIndex = 0;
  let runEnd = (payload[0] >>> 4) + 1;
  for (let y = height - 1; y >= 0; y--) {
    const sourceRow = edge - 1 - Math.min(edge - 1, Math.floor((y + 0.5) * edge / height));
    for (let x = 0; x < width; x++) {
      const sourceCol = Math.min(edge - 1, Math.floor((x + 0.5) * edge / width));
      const cellIndex = sourceRow * edge + sourceCol;
      while (cellIndex >= runEnd) {
        runIndex++;
        runEnd += (payload[runIndex] >>> 4) + 1;
      }
      output[y * width + x] = payload[runIndex] & 0x0f;
    }
  }
  return output;
}

export function parseCrc32(value: string): number {
  if (!/^0x[0-9a-f]{8}$/i.test(value)) throw new Error(`Invalid manifest CRC: ${value}`);
  return Number.parseInt(value.slice(2), 16) >>> 0;
}
