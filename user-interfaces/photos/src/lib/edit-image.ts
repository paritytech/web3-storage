// SPDX-License-Identifier: GPL-3.0-only
//
// M7 — in-browser image editing (browser-only canvas, no React/DOM-state so it
// stays unit-testable). Applies a 90° rotation and/or a crop to a source image
// and re-encodes the result. The edited bytes are then PUT back to the photo's
// path (copy-on-write) and the metadata root re-anchored — see
// `album.state.ts:saveEdit`. Companion to `thumbnail.ts` (same canvas/encode
// path; the `canvasToBlob` wrapper is shared from there).

import { canvasToBlob } from '@/lib/thumbnail'

/** A crop rectangle, in pixels of the *rotated* image (top-left origin). */
export interface CropRect {
  x: number
  y: number
  width: number
  height: number
}

/** Clockwise rotation applied before cropping, in degrees. */
export type Rotation = 0 | 90 | 180 | 270

/** JPEG quality for edited photos (0–1). Higher than thumbnails — this is the full-res blob. */
const EDIT_QUALITY = 0.92

export interface EditedImage {
  bytes: Uint8Array
  contentType: 'image/jpeg'
}

/**
 * Apply `rotation` (clockwise) then `crop` to `source` and re-encode as JPEG.
 *
 * The image is first drawn into an intermediate canvas rotated by `rotation`
 * (its dimensions swap for 90°/270°); `crop` — expressed in that rotated
 * image's pixels — is then copied out into the final canvas. Canvas editing
 * discards the original encoding regardless, so the result is always JPEG.
 *
 * `source` may be an already-decoded `ImageBitmap` (e.g. the editor's live
 * preview, reused to avoid a second decode — the caller still owns and closes
 * it) or raw `Blob`/`Uint8Array` bytes (decoded and closed internally).
 */
export async function applyEdit(
  source: ImageBitmap | Blob | Uint8Array,
  rotation: Rotation,
  crop: CropRect | null,
): Promise<EditedImage> {
  const ownsBitmap = !(source instanceof ImageBitmap)
  const bitmap = source instanceof ImageBitmap ? source : await createImageBitmap(toBlob(source))
  try {
    const rotated = drawRotated(bitmap, rotation)
    const out = crop ? cropCanvas(rotated, crop) : rotated
    const blob = await canvasToBlob(out, 'image/jpeg', EDIT_QUALITY)
    return { bytes: new Uint8Array(await blob.arrayBuffer()), contentType: 'image/jpeg' }
  } finally {
    if (ownsBitmap) bitmap.close()
  }
}

/** Draw `bitmap` rotated clockwise by `rotation` into a fresh canvas sized to the rotated bounds. */
function drawRotated(bitmap: ImageBitmap, rotation: Rotation): HTMLCanvasElement {
  const swap = rotation === 90 || rotation === 270
  const w = swap ? bitmap.height : bitmap.width
  const h = swap ? bitmap.width : bitmap.height

  const canvas = document.createElement('canvas')
  canvas.width = w
  canvas.height = h
  const ctx = canvas.getContext('2d')
  if (!ctx) throw new Error('Could not get a 2D canvas context for the edit')

  // Rotate about the output centre, then draw the source centred on the origin.
  ctx.translate(w / 2, h / 2)
  ctx.rotate((rotation * Math.PI) / 180)
  ctx.drawImage(bitmap, -bitmap.width / 2, -bitmap.height / 2)
  return canvas
}

/** Copy `crop` (clamped to bounds) out of `src` into a new canvas of the crop's size. */
function cropCanvas(src: HTMLCanvasElement, crop: CropRect): HTMLCanvasElement {
  const x = clamp(Math.round(crop.x), 0, src.width)
  const y = clamp(Math.round(crop.y), 0, src.height)
  const width = clamp(Math.round(crop.width), 1, src.width - x)
  const height = clamp(Math.round(crop.height), 1, src.height - y)

  const canvas = document.createElement('canvas')
  canvas.width = width
  canvas.height = height
  const ctx = canvas.getContext('2d')
  if (!ctx) throw new Error('Could not get a 2D canvas context for the crop')
  ctx.drawImage(src, x, y, width, height, 0, 0, width, height)
  return canvas
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(Math.max(v, lo), Math.max(lo, hi))
}

function toBlob(source: Blob | Uint8Array): Blob {
  return source instanceof Blob ? source : new Blob([source as BlobPart])
}
