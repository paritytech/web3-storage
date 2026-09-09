// SPDX-License-Identifier: GPL-3.0-only
//
// Client-side thumbnail generation (browser-only) — the piece the headless flow
// stubbed with placeholder bytes (`scripts/photos-flow.ts` M2). Decode the photo,
// downscale it onto a canvas so the longest edge is ~`MAX_EDGE`px, and re-encode
// as JPEG. The grid renders from these (kilobytes per cell) instead of the
// full-resolution blobs. Per DESIGN.md §"Albums, blobs & the root anchor".

/** Longest-edge target for thumbnails, in CSS pixels. */
export const MAX_EDGE = 320

/** JPEG quality for thumbnails (0–1). */
const THUMB_QUALITY = 0.8

export interface Thumbnail {
  bytes: Uint8Array
  contentType: 'image/jpeg'
}

/**
 * Produce a downscaled JPEG thumbnail for `file`. Decodes via `createImageBitmap`
 * (no DOM image element / load event needed), draws onto an offscreen canvas
 * scaled so the longest edge is at most `MAX_EDGE` (never upscales), and encodes
 * to JPEG. Throws if the browser can't decode the file as an image.
 */
export async function makeThumbnail(file: File | Blob): Promise<Thumbnail> {
  const bitmap = await createImageBitmap(file)
  try {
    const longest = Math.max(bitmap.width, bitmap.height)
    const scale = longest > MAX_EDGE ? MAX_EDGE / longest : 1
    const width = Math.max(1, Math.round(bitmap.width * scale))
    const height = Math.max(1, Math.round(bitmap.height * scale))

    const canvas = document.createElement('canvas')
    canvas.width = width
    canvas.height = height
    const ctx = canvas.getContext('2d')
    if (!ctx) throw new Error('Could not get a 2D canvas context for the thumbnail')
    ctx.drawImage(bitmap, 0, 0, width, height)

    const blob = await canvasToBlob(canvas, 'image/jpeg', THUMB_QUALITY)
    return { bytes: new Uint8Array(await blob.arrayBuffer()), contentType: 'image/jpeg' }
  } finally {
    bitmap.close()
  }
}

/** Promise wrapper over the callback-style `canvas.toBlob`. Shared with `edit-image.ts`. */
export function canvasToBlob(canvas: HTMLCanvasElement, type: string, quality: number): Promise<Blob> {
  return new Promise((resolve, reject) => {
    canvas.toBlob(
      (blob) => (blob ? resolve(blob) : reject(new Error('canvas.toBlob returned null'))),
      type,
      quality,
    )
  })
}
