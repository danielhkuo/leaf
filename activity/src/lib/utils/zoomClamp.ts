/**
 * Pixel size of an image letterboxed inside a frame. With `upscale` an image
 * smaller than the frame is enlarged to fill it: the box then depends only on
 * the image's shape, so a small placeholder and the full photo get the same
 * box and nothing jumps when one replaces the other.
 */
export function fitDimensions(
  frameW: number,
  frameH: number,
  naturalW: number,
  naturalH: number,
  upscale = false,
): { width: number; height: number } {
  if (frameW <= 0 || frameH <= 0 || naturalW <= 0 || naturalH <= 0) {
    return { width: 0, height: 0 };
  }
  const fill = Math.min(frameW / naturalW, frameH / naturalH);
  const fitScale = upscale ? fill : Math.min(fill, 1);
  return {
    width: naturalW * fitScale,
    height: naturalH * fitScale,
  };
}

/** Pan limits for a zoomed image letterboxed inside a frame. */
export function panLimits(
  frameW: number,
  frameH: number,
  naturalW: number,
  naturalH: number,
  scale: number,
  upscale = false,
): { maxX: number; maxY: number } {
  if (frameW <= 0 || frameH <= 0 || naturalW <= 0 || naturalH <= 0 || scale <= 1) {
    return { maxX: 0, maxY: 0 };
  }
  const { width: renderedW, height: renderedH } = fitDimensions(
    frameW,
    frameH,
    naturalW,
    naturalH,
    upscale,
  );
  return {
    maxX: Math.max(0, (renderedW * scale - frameW) / 2),
    maxY: Math.max(0, (renderedH * scale - frameH) / 2),
  };
}

/** Clamp a pan offset to the allowed range. */
export function clampPan(
  tx: number,
  ty: number,
  frameW: number,
  frameH: number,
  naturalW: number,
  naturalH: number,
  scale: number,
  upscale = false,
): { tx: number; ty: number } {
  const { maxX, maxY } = panLimits(frameW, frameH, naturalW, naturalH, scale, upscale);
  return {
    tx: Math.min(maxX, Math.max(-maxX, tx)),
    ty: Math.min(maxY, Math.max(-maxY, ty)),
  };
}
