import { describe, expect, it } from 'vitest';

import {
  createGesture,
  createWheelSwipe,
  isWheelNotch,
  pointerCancel,
  pointerDown,
  pointerMove,
  pointerUp,
  wheelPixels,
  wheelScale,
  wheelSwipe,
  zoomAt,
  type GestureState,
  type View,
} from './gesture';

const LIMITS = { minScale: 1, maxScale: 4 };
const REST: View = { scale: 1, tx: 0, ty: 0 };
const p = (x: number, y: number) => ({ x, y });

/** One finger down at `from`, dragged to `to` and lifted at `now`. */
function stroke(
  state: GestureState,
  from: { x: number; y: number },
  to: { x: number; y: number },
  view: View,
  now: number,
) {
  let s = pointerDown(state, 1, from, view);
  s = pointerMove(s, 1, to, LIMITS).state;
  return pointerUp(s, 1, to, view, now);
}

describe('one finger', () => {
  it('reports a tap, then a double tap at the same place', () => {
    const first = stroke(createGesture(), p(10, 10), p(12, 11), REST, 1_000);
    expect(first.effect).toEqual({ kind: 'tap' });
    const second = stroke(first.state, p(14, 8), p(14, 8), REST, 1_200);
    expect(second.effect).toEqual({ kind: 'double-tap', at: p(14, 8) });
    // The pair is used up: a third tap starts over.
    expect(stroke(second.state, p(14, 8), p(14, 8), REST, 1_300).effect).toEqual({ kind: 'tap' });
  });

  it('does not pair taps that are far apart in time or place', () => {
    const first = stroke(createGesture(), p(0, 0), p(0, 0), REST, 1_000);
    expect(stroke(first.state, p(0, 0), p(0, 0), REST, 1_400).effect).toEqual({ kind: 'tap' });
    expect(stroke(first.state, p(120, 0), p(120, 0), REST, 1_100).effect).toEqual({ kind: 'tap' });
  });

  it('turns the page on a sideways drag of an unzoomed photo', () => {
    expect(stroke(createGesture(), p(100, 0), p(20, 10), REST, 0).effect).toEqual({
      kind: 'swipe',
      direction: 'next',
    });
    expect(stroke(createGesture(), p(0, 0), p(90, -5), REST, 0).effect).toEqual({
      kind: 'swipe',
      direction: 'prev',
    });
  });

  it('ignores short and mostly vertical drags', () => {
    expect(stroke(createGesture(), p(0, 0), p(30, 0), REST, 0).effect).toBeNull();
    expect(stroke(createGesture(), p(0, 0), p(60, 90), REST, 0).effect).toBeNull();
  });

  it('gives drag feedback while unzoomed and leaves the view alone', () => {
    const s = pointerDown(createGesture(), 1, p(0, 0), REST);
    const moved = pointerMove(s, 1, p(-40, 5), LIMITS);
    expect(moved.view).toBeNull();
    expect(moved.drag).toBe(-40);
    expect(pointerMove(s, 1, p(5, 40), LIMITS).drag).toBe(0);
  });

  it('pans a zoomed photo and never turns the page', () => {
    const zoomed: View = { scale: 2, tx: 10, ty: -10 };
    const s = pointerDown(createGesture(), 1, p(0, 0), zoomed);
    const moved = pointerMove(s, 1, p(-80, 20), LIMITS);
    expect(moved.view).toEqual({ scale: 2, tx: -70, ty: 10 });
    expect(moved.drag).toBe(0);
    expect(pointerUp(moved.state, 1, p(-80, 20), moved.view ?? zoomed, 0).effect).toBeNull();
  });

  it('ignores a pointer it never saw go down', () => {
    const s = createGesture();
    expect(pointerMove(s, 9, p(1, 1), LIMITS)).toEqual({ state: s, view: null, drag: 0 });
    expect(pointerUp(s, 9, p(1, 1), REST, 0)).toEqual({ state: s, effect: null });
    expect(pointerCancel(s, 9, REST)).toBe(s);
  });
});

describe('two fingers', () => {
  function pinch(): { state: GestureState; view: View } {
    let s = pointerDown(createGesture(), 1, p(-50, 0), REST);
    s = pointerDown(s, 2, p(50, 0), REST);
    const moved = pointerMove(s, 2, p(150, 0), LIMITS);
    if (!moved.view) throw new Error('a pinch move gives a view');
    return { state: moved.state, view: moved.view };
  }

  it('scales about the point between the fingers', () => {
    const { view } = pinch();
    expect(view.scale).toBe(2);
    // The midpoint moved from 0 to 50, and the photo point under it follows.
    expect(view.tx).toBe(50);
    expect(view.ty).toBe(0);
  });

  it('keeps the scale inside its limits', () => {
    let s = pointerDown(createGesture(), 1, p(-10, 0), REST);
    s = pointerDown(s, 2, p(10, 0), REST);
    expect(pointerMove(s, 2, p(900, 0), LIMITS).view?.scale).toBe(4);
    expect(pointerMove(s, 2, p(-9, 0), LIMITS).view?.scale).toBe(1);
  });

  it('does not jump when one finger lifts: the other pans from where it is', () => {
    const { state, view } = pinch();
    const lifted = pointerUp(state, 2, p(150, 0), view, 0);
    expect(lifted.effect).toBeNull();
    // The remaining finger has not moved yet, so neither does the photo.
    const still = pointerMove(lifted.state, 1, p(-50, 0), LIMITS);
    expect(still.view).toEqual(view);
    const dragged = pointerMove(lifted.state, 1, p(-30, 10), LIMITS);
    expect(dragged.view).toEqual({ scale: 2, tx: view.tx + 20, ty: view.ty + 10 });
  });

  it('ends without a tap or a page turn, however the last finger lifts', () => {
    const { state, view } = pinch();
    const one = pointerUp(state, 2, p(150, 0), view, 0).state;
    // Lifted in place: would otherwise be a tap.
    expect(pointerUp(one, 1, p(-50, 0), view, 10).effect).toBeNull();

    // Pinched back out to no zoom, then the last finger travels far sideways:
    // would otherwise be a swipe.
    let s = pointerDown(createGesture(), 1, p(0, 0), REST);
    s = pointerDown(s, 2, p(100, 0), REST);
    s = pointerUp(s, 2, p(100, 0), REST, 0).state;
    s = pointerMove(s, 1, p(-120, 0), LIMITS).state;
    const end = pointerUp(s, 1, p(-120, 0), REST, 20);
    expect(end.effect).toBeNull();
    // And the next touch is an ordinary one again.
    expect(stroke(end.state, p(100, 0), p(0, 0), REST, 500).effect).toEqual({
      kind: 'swipe',
      direction: 'next',
    });
  });

  it('ignores a third finger', () => {
    const { state } = pinch();
    expect(pointerDown(state, 3, p(0, 90), REST)).toBe(state);
  });
});

describe('a cancelled touch', () => {
  it('does nothing, where lifting at the same place would have turned the page', () => {
    let s = pointerDown(createGesture(), 1, p(100, 0), REST);
    s = pointerMove(s, 1, p(0, 0), LIMITS).state;
    const cancelled = pointerCancel(s, 1, REST);
    expect(cancelled.pointers).toEqual([]);
    // A later lift of the same pointer id is not known any more.
    expect(pointerUp(cancelled, 1, p(0, 0), REST, 0).effect).toBeNull();
  });

  it('leaves the other finger of a pinch panning, with no tap at the end', () => {
    let s = pointerDown(createGesture(), 1, p(-50, 0), REST);
    s = pointerDown(s, 2, p(50, 0), REST);
    const view: View = { scale: 2, tx: 0, ty: 0 };
    s = pointerCancel(s, 2, view);
    expect(pointerMove(s, 1, p(-40, 0), LIMITS).view).toEqual({ scale: 2, tx: 10, ty: 0 });
    expect(pointerUp(s, 1, p(-50, 0), view, 0).effect).toBeNull();
  });
});

describe('zoomAt', () => {
  it('keeps the photo point under the focal point', () => {
    const view = zoomAt(REST, p(100, -40), 2, LIMITS);
    // Photo point = (focal - offset) / scale, before and after.
    expect((100 - view.tx) / view.scale).toBe(100);
    expect((-40 - view.ty) / view.scale).toBe(-40);
  });

  it('recentres at the minimum scale and stops at the maximum', () => {
    expect(zoomAt({ scale: 2, tx: 80, ty: 30 }, p(10, 10), 0.5, LIMITS)).toEqual(REST);
    expect(zoomAt(REST, p(0, 0), 9, LIMITS).scale).toBe(4);
  });
});

describe('wheel', () => {
  it('zooms in proportion to the travel', () => {
    // A trackpad's small deltas barely move the scale; they used to add a
    // fixed step each, which reached the limit in a few events.
    expect(wheelScale(1, -4, false)).toBeCloseTo(1.008, 3);
    expect(wheelScale(1, -100, false)).toBeCloseTo(1.221, 3);
    expect(wheelScale(2, 100, false)).toBeLessThan(2);
    expect(wheelScale(1, -10, true)).toBeGreaterThan(wheelScale(1, -10, false));
  });

  it('reads lines and pages as pixels', () => {
    expect(wheelPixels(3, 0)).toBe(3);
    expect(wheelPixels(3, 1)).toBe(48);
    expect(wheelPixels(1, 2)).toBe(400);
  });

  it('tells a mouse wheel notch from a trackpad scroll', () => {
    expect(isWheelNotch(0, 100, 0)).toBe(true);
    expect(isWheelNotch(0, -3, 1)).toBe(true);
    expect(isWheelNotch(0, 4, 0)).toBe(false);
    expect(isWheelNotch(2, 120, 0)).toBe(false);
  });

  it('turns one page per sideways scroll, ignoring its inertia', () => {
    let s = createWheelSwipe();
    let turned: (string | null)[] = [];
    for (let t = 0; t < 400; t += 16) {
      const r = wheelSwipe(s, 30, t);
      s = r.state;
      turned.push(r.direction);
    }
    expect(turned.filter(Boolean)).toEqual(['next']);

    // After a pause, the next scroll counts again, in either direction.
    turned = [];
    for (let t = 1_000; t < 1_100; t += 16) {
      const r = wheelSwipe(s, -30, t);
      s = r.state;
      turned.push(r.direction);
    }
    expect(turned.filter(Boolean)).toEqual(['prev']);
  });

  it('does not add up scrolls that are separate', () => {
    let s = createWheelSwipe();
    for (const t of [0, 500, 1_000, 1_500]) {
      const r = wheelSwipe(s, 30, t);
      s = r.state;
      expect(r.direction).toBeNull();
    }
  });
});
