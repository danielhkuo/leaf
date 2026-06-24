/** Pan limits for a zoomed image letterboxed inside a frame. */
export function panLimits(
  frameW: number,
  frameH: number,
  naturalW: number,
  naturalH: number,
  scale: number,
): { maxX: number; maxY: number } {
  if (frameW <= 0 || frameH <= 0 || naturalW <= 0 || naturalH <= 0 || scale <= 1) {
    return { maxX: 0, maxY: 0 };
  }
  const fitScale = Math.min(frameW / naturalW, frameH / naturalH, 1);
  const renderedW = naturalW * fitScale;
  const renderedH = naturalH * fitScale;
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
): { tx: number; ty: number } {
  const { maxX, maxY } = panLimits(frameW, frameH, naturalW, naturalH, scale);
  return {
    tx: Math.min(maxX, Math.max(-maxX, tx)),
    ty: Math.min(maxY, Math.max(-maxY, ty)),
  };
}
