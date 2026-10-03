// The photo surface's touch and wheel gestures as a pure state machine: the
// component feeds it pointer events and applies what comes back. Kept out of
// the component so the cases that went wrong on phones can be tested:
// lifting one finger of a pinch must not jump the photo, lifting the second
// must not count as a tap or a swipe, and a cancelled touch does nothing.
//
// Points are relative to the centre of the frame, which is also the origin
// the photo is scaled about. A view is the photo's scale and offset.

export interface Point {
  x: number;
  y: number;
}

export interface View {
  scale: number;
  tx: number;
  ty: number;
}

interface Pointer extends Point {
  id: number;
}

/** What a finished touch asks for. */
export type GestureEffect =
  | { kind: 'tap' }
  | { kind: 'double-tap'; at: Point }
  | { kind: 'swipe'; direction: 'prev' | 'next' };

export interface GestureState {
  readonly pointers: readonly Pointer[];
  /** Where the one-finger drag is measured from. */
  readonly anchor: Point;
  /** The view when the anchor (or the pinch) was set. */
  readonly base: View;
  /** Distance and midpoint of the two fingers when the pinch began. */
  readonly pinchDistance: number;
  readonly pinchMid: Point;
  /**
   * The touch has involved a second finger or a cancelled one. What is left
   * of it can still pan, but its end is neither a tap nor a swipe.
   */
  readonly compound: boolean;
  readonly lastTapAt: number;
  readonly lastTap: Point;
}

export interface GestureLimits {
  minScale: number;
  maxScale: number;
}

/** A finger may wander this far and still be a tap. */
export const TAP_SLOP = 10;
/** Two taps this close in time and place are a double tap. */
export const DOUBLE_TAP_MS = 300;
const DOUBLE_TAP_SLOP = 40;
/** A horizontal drag this long turns the page. */
export const SWIPE_DISTANCE = 50;
/** At or below this the photo counts as not zoomed. */
const UNZOOMED = 1.001;

const ORIGIN: Point = { x: 0, y: 0 };

export function createGesture(): GestureState {
  return {
    pointers: [],
    anchor: ORIGIN,
    base: { scale: 1, tx: 0, ty: 0 },
    pinchDistance: 0,
    pinchMid: ORIGIN,
    compound: false,
    lastTapAt: Number.NEGATIVE_INFINITY,
    lastTap: ORIGIN,
  };
}

export function isZoomed(view: View): boolean {
  return view.scale > UNZOOMED;
}

function clampScale(scale: number, limits: GestureLimits): number {
  return Math.min(limits.maxScale, Math.max(limits.minScale, scale));
}

function distance(a: Point, b: Point): number {
  return Math.hypot(a.x - b.x, a.y - b.y);
}

function midpoint(a: Point, b: Point): Point {
  return { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
}

/**
 * The view after changing the scale so that the part of the photo under
 * `focal` stays under it. Returning to the minimum scale recentres.
 */
export function zoomAt(view: View, focal: Point, scale: number, limits: GestureLimits): View {
  const next = clampScale(scale, limits);
  if (next <= limits.minScale) return { scale: limits.minScale, tx: 0, ty: 0 };
  const ratio = next / view.scale;
  return {
    scale: next,
    tx: focal.x - (focal.x - view.tx) * ratio,
    ty: focal.y - (focal.y - view.ty) * ratio,
  };
}

/** Starts measuring from the fingers that are down now. */
function rebase(state: GestureState, pointers: readonly Pointer[], view: View): GestureState {
  const [a, b] = pointers;
  if (a && b) {
    return {
      ...state,
      pointers,
      base: view,
      pinchDistance: distance(a, b),
      pinchMid: midpoint(a, b),
    };
  }
  return { ...state, pointers, base: view, anchor: a ? { x: a.x, y: a.y } : state.anchor };
}

export function pointerDown(state: GestureState, id: number, at: Point, view: View): GestureState {
  const others = state.pointers.filter((p) => p.id !== id);
  // A third finger is ignored: the pinch stays with the first two.
  if (others.length >= 2) return state;
  const pointers = [...others, { id, ...at }];
  const next = rebase(state, pointers, view);
  return { ...next, compound: pointers.length > 1 };
}

export interface MoveResult {
  state: GestureState;
  /** The view to show, or `null` to leave it. */
  view: View | null;
  /** Horizontal drag of an unzoomed photo, for feedback before a page turn. */
  drag: number;
}

export function pointerMove(
  state: GestureState,
  id: number,
  at: Point,
  limits: GestureLimits,
): MoveResult {
  if (!state.pointers.some((p) => p.id === id)) return { state, view: null, drag: 0 };
  const pointers = state.pointers.map((p) => (p.id === id ? { id, ...at } : p));
  const next = { ...state, pointers };
  const [a, b] = pointers;

  if (a && b) {
    if (state.pinchDistance <= 0) return { state: next, view: null, drag: 0 };
    const scale = clampScale(state.base.scale * (distance(a, b) / state.pinchDistance), limits);
    const mid = midpoint(a, b);
    const ratio = scale / state.base.scale;
    // The photo point that was under the fingers stays under them, so the
    // pinch also drags.
    const view: View = {
      scale,
      tx: mid.x - (state.pinchMid.x - state.base.tx) * ratio,
      ty: mid.y - (state.pinchMid.y - state.base.ty) * ratio,
    };
    return { state: next, view, drag: 0 };
  }

  if (!a) return { state: next, view: null, drag: 0 };
  const dx = a.x - state.anchor.x;
  const dy = a.y - state.anchor.y;
  if (isZoomed(state.base)) {
    return {
      state: next,
      view: { scale: state.base.scale, tx: state.base.tx + dx, ty: state.base.ty + dy },
      drag: 0,
    };
  }
  const sideways = !state.compound && Math.abs(dx) > Math.abs(dy);
  return { state: next, view: null, drag: sideways ? dx : 0 };
}

export interface EndResult {
  state: GestureState;
  effect: GestureEffect | null;
}

/**
 * A finger lifted. `view` is what is on screen now: with one finger left of
 * a pinch, the pan continues from there instead of jumping.
 */
export function pointerUp(
  state: GestureState,
  id: number,
  at: Point,
  view: View,
  now: number,
): EndResult {
  if (!state.pointers.some((p) => p.id === id)) return { state, effect: null };
  const left = state.pointers.filter((p) => p.id !== id);
  if (left.length > 0) {
    return { state: { ...rebase(state, left, view), compound: true }, effect: null };
  }

  const ended: GestureState = { ...state, pointers: [], compound: false };
  if (state.compound) return { state: ended, effect: null };

  const dx = at.x - state.anchor.x;
  const dy = at.y - state.anchor.y;
  if (Math.hypot(dx, dy) < TAP_SLOP) {
    const again =
      now - state.lastTapAt < DOUBLE_TAP_MS && distance(at, state.lastTap) < DOUBLE_TAP_SLOP;
    if (again) {
      return {
        state: { ...ended, lastTapAt: Number.NEGATIVE_INFINITY },
        effect: { kind: 'double-tap', at },
      };
    }
    return { state: { ...ended, lastTapAt: now, lastTap: at }, effect: { kind: 'tap' } };
  }
  if (!isZoomed(state.base) && Math.abs(dx) > SWIPE_DISTANCE && Math.abs(dx) > Math.abs(dy)) {
    return { state: ended, effect: { kind: 'swipe', direction: dx < 0 ? 'next' : 'prev' } };
  }
  return { state: ended, effect: null };
}

/**
 * The browser took the touch away (a system gesture, a scroll, a call).
 * Nothing it was doing counts: no tap, no swipe, no page turn.
 */
export function pointerCancel(state: GestureState, id: number, view: View): GestureState {
  if (!state.pointers.some((p) => p.id === id)) return state;
  const left = state.pointers.filter((p) => p.id !== id);
  if (left.length === 0) return { ...state, pointers: [], compound: false };
  return { ...rebase(state, left, view), compound: true };
}

// --- wheel ---------------------------------------------------------------

/** Pixels a "line" or "page" of wheel travel stands for. */
const LINE_PX = 16;
const PAGE_PX = 400;
/** Zoom per pixel of wheel travel; a trackpad pinch (ctrl + wheel) sends small deltas. */
const WHEEL_ZOOM = 0.002;
const PINCH_ZOOM = 0.01;

/** Wheel travel in pixels, whatever unit the device reports. */
export function wheelPixels(delta: number, deltaMode: number): number {
  if (deltaMode === 1) return delta * LINE_PX;
  if (deltaMode === 2) return delta * PAGE_PX;
  return delta;
}

/**
 * Whether a wheel event looks like one notch of a mouse wheel rather than a
 * trackpad scroll: a mouse sends big, purely vertical steps (or whole
 * lines), a trackpad a stream of small ones. A guess, so both paths have to
 * be harmless when it is wrong.
 */
export function isWheelNotch(deltaX: number, deltaY: number, deltaMode: number): boolean {
  if (deltaX !== 0) return false;
  return deltaMode !== 0 || Math.abs(deltaY) >= 50;
}

/**
 * The scale after a wheel step: proportional to the travel, so a trackpad's
 * stream of small deltas zooms smoothly instead of slamming to the limit.
 */
export function wheelScale(scale: number, deltaY: number, pinch: boolean): number {
  return scale * Math.exp(-deltaY * (pinch ? PINCH_ZOOM : WHEEL_ZOOM));
}

export interface WheelSwipe {
  readonly travel: number;
  readonly lastAt: number;
  /** A page was turned; the rest of this scroll (its inertia) is ignored. */
  readonly spent: boolean;
}

/** A pause this long ends one sideways scroll and starts the next. */
const WHEEL_GAP_MS = 180;
const WHEEL_SWIPE_DISTANCE = 80;

export function createWheelSwipe(): WheelSwipe {
  return { travel: 0, lastAt: Number.NEGATIVE_INFINITY, spent: false };
}

/** Adds sideways wheel travel; one continuous scroll turns at most one page. */
export function wheelSwipe(
  state: WheelSwipe,
  deltaX: number,
  now: number,
): { state: WheelSwipe; direction: 'prev' | 'next' | null } {
  const fresh = now - state.lastAt > WHEEL_GAP_MS;
  const spent = fresh ? false : state.spent;
  const travel = (fresh ? 0 : state.travel) + deltaX;
  if (spent || Math.abs(travel) < WHEEL_SWIPE_DISTANCE) {
    return { state: { travel, lastAt: now, spent }, direction: null };
  }
  return {
    state: { travel: 0, lastAt: now, spent: true },
    direction: travel > 0 ? 'next' : 'prev',
  };
}
