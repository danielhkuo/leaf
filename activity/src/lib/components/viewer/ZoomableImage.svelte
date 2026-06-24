<script lang="ts">
  // Full-screen photo surface: blur-up load (placeholder fades out once the
  // full-res arrives, so no blurred halo lingers), a loading spinner, and
  // zoom/pan. At zoom 1 a horizontal drag navigates days; when zoomed, drag
  // pans and pinch / wheel / the +/- buttons / double-tap change the scale.
  import IconButton from '../ui/IconButton.svelte';
  import Spinner from '../ui/Spinner.svelte';
  import { clampPan } from '../../utils/zoomClamp';

  interface Props {
    src: string;
    placeholder?: string;
    alt: string;
    onPrev?: (() => void) | undefined;
    onNext?: (() => void) | undefined;
  }
  let { src, placeholder, alt, onPrev, onNext }: Props = $props();

  const MAX = 4;
  const STEP = 0.6;
  const DOUBLE_TAP_MS = 300;
  const TAP_SLOP = 10;
  const SWIPE = 50;

  let loaded = $state(false);
  let scale = $state(1);
  let tx = $state(0);
  let ty = $state(0);
  let gesturing = $state(false);
  let frameW = $state(0);
  let frameH = $state(0);
  let naturalW = $state(0);
  let naturalH = $state(0);
  let frameEl: HTMLDivElement | undefined;
  const zoomed = $derived(scale > 1.001);

  const pointers = new Map<number, { x: number; y: number }>();
  let startX = 0;
  let startY = 0;
  let panTx = 0;
  let panTy = 0;
  let pinchDist = 0;
  let pinchScale = 1;
  let lastTapAt = 0;

  // Reset the fade and the zoom whenever the source changes (new day).
  $effect(() => {
    void src;
    loaded = false;
    naturalW = 0;
    naturalH = 0;
    scale = 1;
    tx = 0;
    ty = 0;
  });

  function clamp(): void {
    ({ tx, ty } = clampPan(tx, ty, frameW, frameH, naturalW, naturalH, scale));
  }

  // Re-clamp when the frame or image dimensions change (iframe resize, load).
  $effect(() => {
    if (scale > 1 && naturalW > 0 && naturalH > 0 && frameW > 0 && frameH > 0) clamp();
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

  function zoomTo(next: number): void {
    scale = Math.min(MAX, Math.max(1, next));
    if (scale <= 1) {
      scale = 1;
      tx = 0;
      ty = 0;
    } else {
      clamp();
    }
  }

  function spread(): number {
    const pts = [...pointers.values()];
    const a = pts[0];
    const b = pts[1];
    return a && b ? Math.hypot(a.x - b.x, a.y - b.y) : 0;
  }

  /** True for events on the zoom controls — let the buttons handle them. */
  function onControls(e: PointerEvent): boolean {
    return e.target instanceof Element && e.target.closest('.zoom-controls') !== null;
  }

  function onPointerDown(e: PointerEvent): void {
    if (onControls(e)) return;
    if (e.target instanceof Element) e.target.setPointerCapture?.(e.pointerId);
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    gesturing = true;
    if (pointers.size === 2) {
      pinchDist = spread();
      pinchScale = scale;
    } else {
      startX = e.clientX;
      startY = e.clientY;
      panTx = tx;
      panTy = ty;
    }
  }

  function onPointerMove(e: PointerEvent): void {
    if (!pointers.has(e.pointerId)) return;
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    if (pointers.size >= 2) {
      const d = spread();
      if (pinchDist > 0) zoomTo(pinchScale * (d / pinchDist));
    } else if (zoomed) {
      tx = panTx + (e.clientX - startX);
      ty = panTy + (e.clientY - startY);
      clamp();
    }
  }

  function onPointerUp(e: PointerEvent): void {
    if (!pointers.has(e.pointerId)) return;
    const wasMulti = pointers.size >= 2;
    pointers.delete(e.pointerId);
    if (pointers.size === 0) gesturing = false;
    if (wasMulti) return;

    const dx = e.clientX - startX;
    const dy = e.clientY - startY;
    if (Math.hypot(dx, dy) < TAP_SLOP) {
      // Tap — double-tap toggles zoom.
      const now = Date.now();
      if (now - lastTapAt < DOUBLE_TAP_MS) {
        zoomTo(scale > 1 ? 1 : 2.5);
        lastTapAt = 0;
      } else {
        lastTapAt = now;
      }
    } else if (!zoomed && Math.abs(dx) > SWIPE && Math.abs(dx) > Math.abs(dy)) {
      if (dx < 0) onNext?.();
      else onPrev?.();
    }
  }

  function onWheel(e: WheelEvent): void {
    e.preventDefault();
    zoomTo(scale - Math.sign(e.deltaY) * STEP * 0.5);
  }

  function onImgLoad(e: Event): void {
    const img = e.currentTarget as HTMLImageElement;
    loaded = true;
    naturalW = img.naturalWidth;
    naturalH = img.naturalHeight;
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
  onpointercancel={onPointerUp}
  onwheel={onWheel}
>
  <div
    class="pan"
    class:smooth={!gesturing}
    style="transform:translate({tx}px,{ty}px) scale({scale})"
  >
    {#if placeholder}
      <img
        class="ph"
        class:hidden={loaded}
        src={placeholder}
        alt=""
        aria-hidden="true"
        draggable="false"
      />
    {/if}
    <img
      class="full"
      class:loaded
      {src}
      {alt}
      decoding="async"
      draggable="false"
      onload={onImgLoad}
    />
  </div>

  {#if !loaded}
    <div class="loading"><Spinner size="34px" label="Loading photo" /></div>
  {/if}

  <div class="zoom-controls">
    <IconButton
      ariaLabel="Zoom out"
      variant="overlay"
      disabled={!zoomed}
      onclick={() => zoomTo(scale - STEP)}
    >
      −
    </IconButton>
    <IconButton ariaLabel="Zoom in" variant="overlay" onclick={() => zoomTo(scale + STEP)}>
      +
    </IconButton>
  </div>
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
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    transform-origin: center center;
    will-change: transform;
  }
  .pan.smooth {
    transition: transform var(--motion-base) var(--ease);
  }
  .ph,
  .full {
    grid-area: 1 / 1;
    display: block;
    width: auto;
    height: auto;
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
    user-select: none;
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
  .zoom-controls {
    position: absolute;
    top: var(--space-md);
    right: var(--space-md);
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
