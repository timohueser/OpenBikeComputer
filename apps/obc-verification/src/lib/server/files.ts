import { createHash } from 'node:crypto';
import { createReadStream, promises as fs } from 'node:fs';
import { Readable } from 'node:stream';
import { resolve } from 'node:path';
import { assert } from './domain.ts';
import { store } from './store.ts';
import type { Attachment } from '../types.ts';

export async function boundedBody(request: Request, limit: number): Promise<Buffer> {
  assert(Number(request.headers.get('content-length') ?? 0) <= limit, 'Upload is too large.', 413);
  assert(request.body, 'Request body is required.');
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = []; let length = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read(); if (done) break;
      length += value.byteLength;
      if (length > limit) { await reader.cancel(); assert(false, 'Upload is too large.', 413); }
      chunks.push(value);
    }
  } finally { reader.releaseLock(); }
  return Buffer.concat(chunks, length);
}
export async function upload(request: Request): Promise<Attachment> {
  const limit = Number(process.env.VERIFICATION_UPLOAD_LIMIT_BYTES || 67108864);
  const body = await boundedBody(request, limit + 65536);
  const form = await new Response(new Uint8Array(body), { headers: { 'Content-Type': request.headers.get('content-type') || '' } }).formData();
  const entries = [...form.entries()];
  assert(entries.length === 1 && entries[0][0] === 'file' && entries[0][1] instanceof File, 'Upload one file using the file field.');
  const file = entries[0][1] as File;
  assert(file.size > 0 && file.size <= limit, 'File is empty or too large.', 413);
  const content = Buffer.from(await file.arrayBuffer());
  const record: Attachment = { id: store().id(), name: file.name.split(/[\\/]/).pop()!.replace(/[\x00-\x1f\x7f]/g, '').slice(0, 200) || 'attachment', size: content.length, sha256: createHash('sha256').update(content).digest('hex') };
  const location = resolve(store().directory, 'files', record.id);
  await fs.writeFile(location, content, { flag: 'wx', mode: 0o600 });
  store().put('file', record.id, record);
  return record;
}
export function download(id: string): Response {
  const file = store().file(id);
  return new Response(Readable.toWeb(createReadStream(resolve(store().directory, 'files', file.id))) as ReadableStream, { headers: {
    'Content-Type': 'application/octet-stream', 'Content-Length': String(file.size),
    'Content-Disposition': `attachment; filename*=UTF-8''${encodeURIComponent(file.name)}`, 'ETag': `"${file.sha256}"`, 'Content-Security-Policy': "default-src 'none'; sandbox"
  } });
}
