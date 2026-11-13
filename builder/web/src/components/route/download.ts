/** Saves a file that the page made, named after a route or a ride. */
export function download(content: BlobPart, type: string, name: string, extension: string): void {
    const url = URL.createObjectURL(new Blob([content], { type }));
    const link = document.createElement('a');
    link.href = url;
    link.download = `${name.replace(/[^\p{L}\p{N} _-]/gu, '').slice(0, 80) || 'route'}.${extension}`;
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
}
