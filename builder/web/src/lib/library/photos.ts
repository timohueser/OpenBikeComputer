/** Makes the copy and the small picture of a photo with a canvas, and adds the photo to its ride. */
import type { PhotoEntry } from './records';
import type { Library } from './store';

/** The longest side of the copy and of the small picture, and the quality of their JPEG files. */
const COPY_PX = 2048, SMALL_PX = 160, QUALITY = 0.85;

/** A JPEG of an open image, with a longest side of `longest` pixels at most. */
function scaled(image: ImageBitmap, longest: number): Promise<Blob> {
    const scale = Math.min(1, longest / Math.max(image.width, image.height));
    const canvas = new OffscreenCanvas(Math.max(1, Math.round(image.width * scale)), Math.max(1, Math.round(image.height * scale)));
    const context = canvas.getContext('2d')!;
    context.imageSmoothingQuality = 'high';
    context.drawImage(image, 0, 0, canvas.width, canvas.height);
    return canvas.convertToBlob({ type: 'image/jpeg', quality: QUALITY });
}

/** The small picture of an image file. It throws when the browser cannot open the file. */
export async function smallPicture(file: Blob): Promise<Blob> {
    const image = await createImageBitmap(file);
    try { return await scaled(image, SMALL_PX); } finally { image.close(); }
}

/** Adds the photo of a file to a ride, at the place `at`. False when the ride already has this file. It opens the
 * file one time, so the memory holds one open photo. It throws when the browser cannot open the file, or when it has
 * no room for the copy (`QuotaExceededError`). */
export async function addPhoto(library: Library, ride: string, file: Blob, at: Pick<PhotoEntry, 'time' | 'place'>): Promise<boolean> {
    const hash = new Uint8Array(await crypto.subtle.digest('SHA-256', await file.arrayBuffer())).subarray(0, 12);
    const id = `${ride}-${Array.from(hash, byte => byte.toString(16).padStart(2, '0')).join('')}`;
    const image = await createImageBitmap(file);
    try {
        return await library.addPhoto({ id, ride, ...at, image: await scaled(image, COPY_PX), small: await scaled(image, SMALL_PX) });
    } finally { image.close(); }
}
