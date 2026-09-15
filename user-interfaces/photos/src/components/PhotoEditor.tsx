// SPDX-License-Identifier: GPL-3.0-only
//
// M7 — in-browser crop/rotate editor. Opened from the Lightbox for the photo
// currently in view; renders the full-resolution image (reusing the lightbox's
// object URL — no second download) onto a canvas, lets the user rotate in 90°
// steps and drag a crop rectangle, then composes the result via `applyEdit` and
// hands the edited bytes to `saveEdit`. Save re-PUTs to the *same* path
// (copy-on-write) and re-anchors the metadata root.

import { useCallback, useEffect, useRef, useState } from 'react'
import { RotateCcw, RotateCw, RefreshCw, X } from 'lucide-react'
import {
  useEditorOpen,
  useLightbox,
  closeEditor,
  saveEdit,
  type LightboxState,
} from '@/state/album.state'
import { applyEdit, type CropRect, type Rotation } from '@/lib/edit-image'
import { Button } from '@/components/ui/Button'
import { Spinner } from '@/components/ui/Spinner'

/** A rectangle in displayed (CSS) pixels relative to the canvas box. */
interface Rect {
  x: number
  y: number
  w: number
  h: number
}

type DragMode = 'new' | 'move' | 'nw' | 'ne' | 'sw' | 'se'
interface DragState {
  mode: DragMode
  startX: number
  startY: number
  orig: Rect
}

/** Hit radius (px) for grabbing a crop corner. */
const HANDLE = 14
/** Drags smaller than this (px) are treated as a click and clear the crop. */
const MIN_CROP = 8

export function PhotoEditor() {
  const open = useEditorOpen()
  const photo = useLightbox()
  if (!open || !photo) return null
  // Remount (resetting rotation/crop and reloading the bitmap) when the photo changes.
  return <Editor key={photo.url} photo={photo} />
}

function Editor({ photo }: { photo: LightboxState }) {
  const [rotation, setRotation] = useState<Rotation>(0)
  const [crop, setCrop] = useState<Rect | null>(null)
  const [saving, setSaving] = useState(false)

  const canvasRef = useRef<HTMLCanvasElement>(null)
  const bitmapRef = useRef<ImageBitmap | null>(null)
  const rotationRef = useRef<Rotation>(0)
  const dragRef = useRef<DragState | null>(null)

  // Draw the loaded bitmap rotated by the current rotation; the canvas takes the
  // rotated image's pixel dimensions (CSS scales it down to fit the viewport).
  const drawCurrent = useCallback(() => {
    const bitmap = bitmapRef.current
    const canvas = canvasRef.current
    if (!bitmap || !canvas) return
    const rot = rotationRef.current
    const swap = rot === 90 || rot === 270
    canvas.width = swap ? bitmap.height : bitmap.width
    canvas.height = swap ? bitmap.width : bitmap.height
    const ctx = canvas.getContext('2d')
    if (!ctx) return
    ctx.translate(canvas.width / 2, canvas.height / 2)
    ctx.rotate((rot * Math.PI) / 180)
    ctx.drawImage(bitmap, -bitmap.width / 2, -bitmap.height / 2)
  }, [])

  // Decode the full-resolution photo from the lightbox object URL (local; no
  // network round-trip) and draw it. Cleans up the bitmap on unmount.
  useEffect(() => {
    let cancelled = false
    void (async () => {
      const blob = await (await fetch(photo.url)).blob()
      const bitmap = await createImageBitmap(blob)
      if (cancelled) {
        bitmap.close()
        return
      }
      bitmapRef.current = bitmap
      drawCurrent()
    })()
    return () => {
      cancelled = true
      bitmapRef.current?.close()
      bitmapRef.current = null
    }
  }, [photo.url, drawCurrent])

  // Re-draw and drop the crop whenever the rotation changes (the crop is in the
  // pre-rotation frame, so it no longer maps cleanly).
  useEffect(() => {
    rotationRef.current = rotation
    drawCurrent()
    setCrop(null)
  }, [rotation, drawCurrent])

  // Escape cancels the edit (returns to the lightbox).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !saving) closeEditor()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [saving])

  const rotateLeft = () => setRotation((r) => (((r + 270) % 360) as Rotation))
  const rotateRight = () => setRotation((r) => (((r + 90) % 360) as Rotation))
  const resetEdits = () => {
    setRotation(0)
    setCrop(null)
  }

  // ── Crop pointer interaction (pointer-captured on the interaction layer) ──

  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (saving) return
    const box = e.currentTarget.getBoundingClientRect()
    const px = e.clientX - box.left
    const py = e.clientY - box.top
    let mode: DragMode = 'new'
    if (crop) {
      const corner = hitCorner(crop, px, py)
      if (corner) mode = corner
      else if (px >= crop.x && px <= crop.x + crop.w && py >= crop.y && py <= crop.y + crop.h)
        mode = 'move'
    }
    const orig = crop ?? { x: px, y: py, w: 0, h: 0 }
    if (mode === 'new') setCrop({ x: px, y: py, w: 0, h: 0 })
    dragRef.current = { mode, startX: px, startY: py, orig }
    e.currentTarget.setPointerCapture(e.pointerId)
  }

  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const d = dragRef.current
    if (!d) return
    const box = e.currentTarget.getBoundingClientRect()
    const px = clamp(e.clientX - box.left, 0, box.width)
    const py = clamp(e.clientY - box.top, 0, box.height)
    if (d.mode === 'new') {
      setCrop(rectFromPoints(d.startX, d.startY, px, py))
    } else if (d.mode === 'move') {
      const dx = px - d.startX
      const dy = py - d.startY
      setCrop({
        ...d.orig,
        x: clamp(d.orig.x + dx, 0, box.width - d.orig.w),
        y: clamp(d.orig.y + dy, 0, box.height - d.orig.h),
      })
    } else {
      setCrop(resizeCorner(d.orig, d.mode, px, py))
    }
  }

  const onPointerUp = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!dragRef.current) return
    dragRef.current = null
    if (e.currentTarget.hasPointerCapture(e.pointerId)) {
      e.currentTarget.releasePointerCapture(e.pointerId)
    }
    // A tiny drag (or a plain click on empty space) clears the crop.
    setCrop((c) => (c && (c.w < MIN_CROP || c.h < MIN_CROP) ? null : c))
  }

  const onSave = async () => {
    const bitmap = bitmapRef.current
    const canvas = canvasRef.current
    if (!bitmap || !canvas) return
    setSaving(true)
    try {
      const natCrop = toNaturalCrop(crop, canvas)
      const edited = await applyEdit(bitmap, rotation, natCrop)
      // On success `saveEdit` closes the editor + lightbox, unmounting this component.
      await saveEdit(edited.bytes, edited.contentType)
    } catch {
      // `saveEdit` surfaces FS/anchor failures via the library error banner; keep
      // the editor open so the user can retry.
      setSaving(false)
    }
  }

  return (
    <div
      className="fixed inset-0 z-[60] flex flex-col items-center justify-between gap-4 bg-black/85 p-6"
      data-testid="photo-editor"
    >
      {/* Toolbar */}
      <div className="flex w-full max-w-3xl items-center justify-between">
        <div className="flex items-center gap-2">
          <Button
            size="sm"
            variant="outline"
            onClick={rotateLeft}
            disabled={saving}
            data-testid="editor-rotate-left"
          >
            <RotateCcw className="mr-1.5 h-4 w-4" /> Rotate left
          </Button>
          <Button
            size="sm"
            variant="outline"
            onClick={rotateRight}
            disabled={saving}
            data-testid="editor-rotate-right"
          >
            <RotateCw className="mr-1.5 h-4 w-4" /> Rotate right
          </Button>
          <Button size="sm" variant="ghost" onClick={resetEdits} disabled={saving}>
            <RefreshCw className="mr-1.5 h-4 w-4" /> Reset
          </Button>
        </div>
        <button
          type="button"
          onClick={() => !saving && closeEditor()}
          aria-label="Cancel"
          className="rounded-md p-2 text-gray-300 hover:bg-white/10 hover:text-white disabled:opacity-50"
          disabled={saving}
        >
          <X className="h-6 w-6" />
        </button>
      </div>

      {/* Canvas + crop overlay */}
      <div className="relative flex min-h-0 flex-1 items-center justify-center">
        <div className="relative inline-block">
          <canvas
            ref={canvasRef}
            className="block max-h-[68vh] max-w-full rounded-md"
            data-testid="editor-canvas"
          />
          <div
            className="absolute inset-0 cursor-crosshair touch-none overflow-hidden"
            onPointerDown={onPointerDown}
            onPointerMove={onPointerMove}
            onPointerUp={onPointerUp}
            data-testid="editor-crop-layer"
          >
            {crop && (
              <div
                className="absolute border border-purple-400"
                style={{
                  left: crop.x,
                  top: crop.y,
                  width: crop.w,
                  height: crop.h,
                  boxShadow: '0 0 0 9999px rgba(0,0,0,0.55)',
                }}
                data-testid="editor-crop-box"
              >
                {(['nw', 'ne', 'sw', 'se'] as const).map((c) => (
                  <span
                    key={c}
                    className="pointer-events-none absolute h-3 w-3 rounded-full border border-white bg-purple-500"
                    style={cornerStyle(c)}
                  />
                ))}
              </div>
            )}
          </div>
        </div>
      </div>

      {/* Footer */}
      <div className="flex w-full max-w-3xl items-center justify-between">
        <p className="text-xs text-gray-400">
          {crop ? 'Drag the rectangle or its corners to crop.' : 'Drag on the photo to crop.'}{' '}
          Saving replaces this photo (the original blob lingers — copy-on-write).
        </p>
        <div className="flex items-center gap-2">
          <Button variant="ghost" onClick={() => closeEditor()} disabled={saving}>
            Cancel
          </Button>
          <Button onClick={() => void onSave()} disabled={saving} data-testid="editor-save">
            {saving ? (
              <>
                <Spinner size="sm" className="mr-2" /> Saving…
              </>
            ) : (
              'Save'
            )}
          </Button>
        </div>
      </div>
    </div>
  )
}

// ─────────────────────────────────────────────────────────────────────────────
// Geometry helpers
// ─────────────────────────────────────────────────────────────────────────────

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(Math.max(v, lo), Math.max(lo, hi))
}

function rectFromPoints(x0: number, y0: number, x1: number, y1: number): Rect {
  return { x: Math.min(x0, x1), y: Math.min(y0, y1), w: Math.abs(x1 - x0), h: Math.abs(y1 - y0) }
}

/** Resize by dragging `corner` to (px,py), keeping the opposite corner fixed. */
function resizeCorner(orig: Rect, corner: DragMode, px: number, py: number): Rect {
  const left = orig.x
  const right = orig.x + orig.w
  const top = orig.y
  const bottom = orig.y + orig.h
  const fixedX = corner === 'nw' || corner === 'sw' ? right : left
  const fixedY = corner === 'nw' || corner === 'ne' ? bottom : top
  return rectFromPoints(fixedX, fixedY, px, py)
}

/** Return the corner of `c` within `HANDLE` px of (px,py), if any. */
function hitCorner(c: Rect, px: number, py: number): DragMode | null {
  const corners: Array<[DragMode, number, number]> = [
    ['nw', c.x, c.y],
    ['ne', c.x + c.w, c.y],
    ['sw', c.x, c.y + c.h],
    ['se', c.x + c.w, c.y + c.h],
  ]
  for (const [mode, cx, cy] of corners) {
    if (Math.abs(px - cx) <= HANDLE && Math.abs(py - cy) <= HANDLE) return mode
  }
  return null
}

function cornerStyle(c: 'nw' | 'ne' | 'sw' | 'se'): React.CSSProperties {
  const off = -6
  return {
    left: c === 'nw' || c === 'sw' ? off : undefined,
    right: c === 'ne' || c === 'se' ? off : undefined,
    top: c === 'nw' || c === 'ne' ? off : undefined,
    bottom: c === 'sw' || c === 'se' ? off : undefined,
  }
}

/** Map a display-pixel crop to the canvas's natural (rotated) pixels for `applyEdit`. */
function toNaturalCrop(crop: Rect | null, canvas: HTMLCanvasElement): CropRect | null {
  if (!crop) return null
  const box = canvas.getBoundingClientRect()
  if (box.width === 0 || box.height === 0) return null
  const sx = canvas.width / box.width
  const sy = canvas.height / box.height
  return { x: crop.x * sx, y: crop.y * sy, width: crop.w * sx, height: crop.h * sy }
}
