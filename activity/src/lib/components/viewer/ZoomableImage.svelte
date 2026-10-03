<script lang="ts">
  // Full-screen photo surface: blur-up load (the placeholder already fills
  // the box the photo will take, then fades out), a loading spinner, a
  // failure state, and zoom/pan. Unzoomed, a sideways drag or scroll turns
  // the page; zoomed, a drag pans. Pinch, ctrl + wheel, a mouse wheel, the
  // +/- buttons and double-tap change the scale about the point under them.
  // The touch rules live in utils/gesture.ts.
  import Button from '../ui/Button.svelte';
  import IconButton from '../ui/IconButton.svelte';
  import Spinner from '../ui/Spinner.svelte';
  import {
    createGesture,
    createWheelSwipe,
    isWheelNotch,
    isZoomed,
    pointerCancel,
    pointerDown,
    pointerMove,
    pointerUp,
    wheelPixels,
    wheelScale,
    wheelSwipe,
    zoomAt,
    type Point,
    type View,
  } from '../../utils/gesture';
  import { clampPan, fitDimensions } from '../../utils/zoomClamp';

  interface Props {
    src: string;
    placeholder?: string | undefined;
    alt: string;
    onPrev?: (() => void) | undefined;
    onNext?: (() => void) | undefined;
    /** Called before a failed photo is tried again (to renew its address). */
    onRetry?: (() => void) | undefined;
  }
  let { src, placeholder, alt, onPrev, onNext, onRetry }: Props = $props();

  const LIMITS = { minScale: 1, maxScale: 4 };
  const STEP = 0.6;
  const DOUBLE_TAP_SCALE = 2.5;
  /** How far the photo follows a drag towards a side with no page. */
  const DEAD_END_DRAG = 0.3;
  const REST: View = { scale: 1, tx: 0, ty: 0 };

  let loaded = $state(false);
  let failed = $state(false);
  let attempt = $state(0);
  let view = $state<View>(REST);
  let drag = $state(0);
  let gesturing = $state(false);
  let frameW = $state(0);
  let frameH = $state(0);
  let naturalW = $state(0);
  let naturalH = $state(0);
  let frameEl: HTMLDivElement | undefined;
  let fullImg = $state<HTMLImageElement>();
  const zoomed = $derived(isZoomed(view));
  // Filled to the frame whatever the pixel size, so the 256px placeholder
  // and the photo share one box.
  const display = $derived(fitDimensions(frameW, frameH, naturalW, naturalH, true));

  let gesture = createGesture();
  let wheel = createWheelSwipe();

  function syncNatural(img: HTMLImageElement, markLoaded = false): void {
    if (img.naturalWidth <= 0 || img.naturalHeight <= 0) return;
    // The placeholder set the box; the photo keeps it unless its shape differs.
    if (naturalW <= 0 || markLoaded) {
      naturalW = img.naturalWidth;
      naturalH = img.naturalHeight;
    }
    if (markLoaded) loaded = true;
  }

  // Reset the fade, the zoom and any failure whenever the source changes.
  $effect(() => {
    void src;
    loaded = false;
    failed = false;
    naturalW = 0;
    naturalH = 0;
    view = REST;
    drag = 0;
  });

  // Cached images may be complete before onload fires.
  $effect(() => {
    void src;
    if (fullImg?.complete) syncNatural(fullImg, true);
  });

  function clamped(next: View): View {
    if (!isZoomed(next)) return REST;
    const { tx, ty } = clampPan(
      next.tx,
      next.ty,
      frameW,
      frameH,
      naturalW,
      naturalH,
      next.scale,
      true,
    );
    return { scale: next.scale, tx, ty };
  }

  // Re-clamp when the frame or image dimensions change (iframe resize, load).
  $effect(() => {
    if (view.scale > 1 && naturalW > 0 && naturalH > 0 && frameW > 0 && frameH > 0) {
      const next = clamped(view);
      if (next.tx !== view.tx || next.ty !== view.ty) view = next;
    }
  });

  $effect(() => {
    const el = frameEl;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      frameW = el.clientWidth;
      frameH = el.clientHeight;
    });
    ro.observe(el);
    return () => ro.disconnect();
  });

  /** The event's position relative to the frame's centre. */
  function at(e: { clientX: number; clientY: number }): Point {
    const rect = frameEl?.getBoundingClientRect();
    if (!rect) return { x: 0, y: 0 };
    return {
      x: e.clientX - rect.left - rect.width / 2,
      y: e.clientY - rect.top - rect.height / 2,
    };
  }

  function zoomTo(scale: number, focal: Point = { x: 0, y: 0 }): void {
    view = clamped(zoomAt(view, focal, scale, LIMITS));
  }

  function turn(direction: 'prev' | 'next'): void {
    (direction === 'next' ? onNext : onPrev)?.();
  }

  /** True for events on the buttons — let them handle those. */
  function onControls(e: Event): boolean {
    return e.target instanceof Element && e.target.closest('button') !== null;
  }

  // A photo that failed still tracks the touch, so a swipe turns the page;
  // there is nothing to zoom or pan, so those effects are skipped.
  function onPointerDown(e: PointerEvent): void {
    if (onControls(e)) return;
    if (e.pointerType === 'mouse' && e.button !== 0) return;
    if (e.target instanceof Element) e.target.setPointerCapture?.(e.pointerId);
    gesture = pointerDown(gesture, e.pointerId, at(e), view);
    gesturing = gesture.pointers.length > 0;
  }

  function onPointerMove(e: PointerEvent): void {
    const moved = pointerMove(gesture, e.pointerId, at(e), LIMITS);
    gesture = moved.state;
    if (moved.view && !failed) view = clamped(moved.view);
    const open = moved.drag < 0 ? onNext : onPrev;
    drag = open ? moved.drag : moved.drag * DEAD_END_DRAG;
  }

  function onPointerUp(e: PointerEvent): void {
    const point = at(e);
    const ended = pointerUp(gesture, e.pointerId, point, view, Date.now());
    gesture = ended.state;
    gesturing = gesture.pointers.length > 0;
    if (!gesturing) drag = 0;
    const effect = ended.effect;
    if (effect?.kind === 'double-tap') {
      if (!failed) zoomTo(zoomed ? 1 : DOUBLE_TAP_SCALE, effect.at);
    } else if (effect?.kind === 'swipe') {
      turn(effect.direction);
    }
  }

  // The browser took the touch (a system gesture, say): clear it, do nothing.
  function onPointerCancel(e: PointerEvent): void {
    gesture = pointerCancel(gesture, e.pointerId, view);
    gesturing = gesture.pointers.length > 0;
    if (!gesturing) drag = 0;
  }

  /** A sideways scroll turns the page, once per scroll. */
  function wheelTurn(dx: number): void {
    const swiped = wheelSwipe(wheel, dx, Date.now());
    wheel = swiped.state;
    if (swiped.direction) turn(swiped.direction);
  }

  function onWheel(e: WheelEvent): void {
    e.preventDefault();
    const dx = wheelPixels(e.deltaX, e.deltaMode);
    const dy = wheelPixels(e.deltaY, e.deltaMode);
    const sideways = Math.abs(dx) > Math.abs(dy);
    if (failed) {
      // Nothing to zoom or pan: only a sideways scroll means anything.
      if (sideways) wheelTurn(dx);
    } else if (e.ctrlKey) {
      // A trackpad pinch arrives as ctrl + wheel.
      zoomTo(wheelScale(view.scale, dy, true), at(e));
    } else if (isWheelNotch(e.deltaX, e.deltaY, e.deltaMode)) {
      zoomTo(wheelScale(view.scale, dy, false), at(e));
    } else if (zoomed) {
      view = clamped({ scale: view.scale, tx: view.tx - dx, ty: view.ty - dy });
    } else if (sideways) {
      wheelTurn(dx);
    }
    // A vertical trackpad scroll over an unzoomed photo does nothing.
  }

  function onImgLoad(e: Event): void {
    syncNatural(e.currentTarget as HTMLImageElement, true);
  }

  function onImgError(): void {
    failed = true;
    view = REST;
  }

  function onPlaceholderLoad(e: Event): void {
    if (naturalW > 0) return;
    syncNatural(e.currentTarget as HTMLImageElement, false);
  }

  function retry(): void {
    onRetry?.();
    failed = false;
    loaded = false;
    attempt += 1;
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="frame"
  class:zoomed
  bind:this={frameEl}
  bind:clientWidth={frameW}
  bind:clientHeight={frameH}
  onpointerdown={onPointerDown}
  onpointermove={onPointerMove}
  onpointerup={onPointerUp}
  onpointercancel={onPointerCancel}
  onwheel={onWheel}
>
  {#if failed}
    <div class="failed" role="alert">
      <p>This photo didn’t load.</p>
      <Button size="sm" variant="primary" onclick={retry}>Try again</Button>
    </div>
  {:else}
    <div
      class="pan"
      class:fit={display.width > 0}
      class:fallback={display.width <= 0}
      class:smooth={!gesturing}
      style:transform="translate({view.tx + drag}px,{view.ty}px) scale({view.scale})"
      style:width={display.width > 0 ? `${display.width}px` : null}
      style:height={display.height > 0 ? `${display.height}px` : null}
    >
      {#if placeholder}
        <img
          class="ph"
          class:hidden={loaded}
          src={placeholder}
          alt=""
          aria-hidden="true"
          draggable="false"
          onload={onPlaceholderLoad}
        />
      {/if}
      {#key attempt}
        <img
          bind:this={fullImg}
          class="full"
          class:loaded
          {src}
          {alt}
          decoding="async"
          draggable="false"
          onload={onImgLoad}
          onerror={onImgError}
        />
      {/key}
    </div>

    {#if !loaded}
      <div class="loading"><Spinner size="34px" label="Loading photo" /></div>
    {/if}

    <div class="zoom-controls">
      <IconButton
        ariaLabel="Zoom out"
        variant="overlay"
        disabled={!zoomed}
        onclick={() => zoomTo(view.scale - STEP)}
      >
        −
      </IconButton>
      <IconButton
        ariaLabel="Zoom in"
        variant="overlay"
        disabled={view.scale >= LIMITS.maxScale}
        onclick={() => zoomTo(view.scale + STEP)}
      >
        +
      </IconButton>
    </div>
  {/if}
</div>

<style>
  .frame {
    position: relative;
    width: 100%;
    height: 100%;
    min-height: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
    touch-action: none;
  }
  .frame.zoomed {
    cursor: grab;
  }
  .frame.zoomed:active {
    cursor: grabbing;
  }
  .pan {
    position: relative;
    flex-shrink: 0;
    display: grid;
    transform-origin: center center;
    will-change: transform;
  }
  .pan.fallback {
    position: absolute;
    inset: 0;
    place-items: center;
  }
  .pan.fallback .ph,
  .pan.fallback .full {
    width: auto;
    height: auto;
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
  }
  .pan.smooth {
    transition: transform var(--motion-base) var(--ease);
  }
  .ph,
  .full {
    grid-area: 1 / 1;
    display: block;
    width: 100%;
    height: 100%;
    user-select: none;
    -webkit-user-drag: none;
  }
  .ph {
    filter: blur(14px);
    transition: opacity var(--motion-base) var(--ease);
  }
  /* Once the full-res arrives, fade the placeholder fully out — no halo. */
  .ph.hidden {
    opacity: 0;
  }
  .full {
    opacity: 0;
    transition: opacity var(--motion-base) var(--ease);
  }
  .full.loaded {
    opacity: 1;
  }
  .loading {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    pointer-events: none;
  }
  .failed {
    display: grid;
    gap: var(--space-sm);
    justify-items: center;
    padding: var(--space-md);
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
    text-align: center;
  }
  .failed p {
    margin: 0;
  }
  .zoom-controls {
    position: absolute;
    top: var(--space-xs);
    right: 0;
    display: flex;
    gap: var(--space-xs);
  }
  @media (prefers-reduced-motion: reduce) {
    .pan.smooth,
    .ph,
    .full {
      transition: none;
    }
  }
</style>
