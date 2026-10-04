<script lang="ts">
  // While Discord has leaf shrunk to a tile (stores/layout.svelte.ts), the
  // screens inside are put away and one small card fills the tile instead:
  // a cropped corner of a screen made for a phone says nothing. The screens
  // stay mounted, so what they hold (the day the viewer is on, a form half
  // filled in, the listener that refreshes on return) is there when the tile
  // is opened again. The card has nothing to press: a tap on the tile is
  // Discord's, and opens leaf. When something is waiting behind the card
  // (a press in chat, see Gallery.svelte) it says so in words, the `cue`.
  import type { Snippet } from 'svelte';

  import { layout, measureViewport, watchViewport } from '../../stores/layout.svelte';

  interface Props {
    children: Snippet;
    /** An emoji for the card: a series' own, or leaf's mark. */
    mark?: string | undefined;
    /** The card's name: a series', or leaf's. */
    title?: string | undefined;
    /** A word or two under the name: the day, or what leaf is doing. */
    detail?: string | undefined;
    /** A thumbnail to fill the tile with, behind the words. */
    image?: string | null | undefined;
    /** What a tap on the tile will do, when it is worth saying: "Tap to open". */
    cue?: string | undefined;
  }
  let { children, mark = '🍃', title = 'leaf', detail, image = null, cue }: Props = $props();

  $effect(() => watchViewport());

  // Putting the screens away takes the page's scroll and whatever had focus
  // with them. Both are kept on the way in and put back on the way out.
  let kept: { scrollY: number; focused: Element | null } | null = null;

  // Where the page was scrolled to is noted as it scrolls, not when the tile
  // comes up: by the time a resize is reported, WebKit has laid the page out
  // at the tile's width and moved it. That move is a scroll too, and comes
  // first, so each scroll checks the size it happened at.
  let scrolledTo = 0;
  $effect(() => {
    const note = (): void => {
      measureViewport();
      if (!kept && !layout.tile) scrolledTo = window.scrollY;
    };
    window.addEventListener('scroll', note, { passive: true });
    return () => window.removeEventListener('scroll', note);
  });

  let screensEl: HTMLElement | undefined;

  $effect.pre(() => {
    if (!layout.tile || kept) return;
    kept = { scrollY: scrolledTo, focused: document.activeElement };
    // A tile has no controls: a video left playing could not be stopped,
    // and would be heard from behind the card. It stays paused on return.
    for (const video of screensEl?.querySelectorAll('video') ?? []) video.pause();
  });
  $effect(() => {
    if (layout.tile || !kept) return;
    const { scrollY, focused } = kept;
    kept = null;
    window.scrollTo(0, scrollY);
    if (focused instanceof HTMLElement && focused.isConnected) {
      focused.focus({ preventScroll: true });
    }
    // A dialog that opened behind the card (a press in chat) could not take
    // focus while it was inert: it gets it now, not the page under it.
    if (document.activeElement === document.body) {
      screensEl?.querySelector<HTMLElement>('[role="dialog"]')?.focus({ preventScroll: true });
    }
  });
</script>

<!-- No box of its own, so the screens lay out exactly as without it. -->
<div class="screens" bind:this={screensEl} hidden={layout.tile} inert={layout.tile}>
  {@render children()}
</div>

{#if layout.tile}
  <!-- The page, for as long as it is this small: the screens' own <main> is
       inert with the rest of them. -->
  <main class="tile" class:pictured={image !== null} class:cued={!!cue}>
    {#if image !== null}
      <img class="picture" src={image} alt="" decoding="async" draggable="false" />
    {/if}
    <div class="card">
      <span class="mark" aria-hidden="true">{mark}</span>
      <h1>{title}</h1>
      {#if detail}<p>{detail}</p>{/if}
      {#if cue}<p class="cue">{cue}</p>{/if}
    </div>
  </main>
{/if}

<style>
  .screens {
    display: contents;
  }
  .screens[hidden] {
    display: none;
  }

  /* Fixed to the viewport, clear of #app's safe-area padding: an inset
   * measured for the full screen would swallow a tile. Above the day viewer
   * and the shell's notice. */
  .tile {
    /* Discord shows a square cut from the middle of the page, not the whole
     * of it: on Android the page is 120 x 214 and the tile is its middle
     * 120 x 120. Only the picture runs to the page's edges; the words stay
     * inside that square. */
    --cut-x: max(0px, (100vw - 100vh) / 2);
    --cut-y: max(0px, (100vh - 100vw) / 2);
    position: fixed;
    inset: 0;
    z-index: 30;
    display: grid;
    place-items: center;
    padding: calc(var(--space-xs) + var(--cut-y)) calc(var(--space-xs) + var(--cut-x));
    overflow: hidden;
    background: var(--canvas);
  }
  .picture {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
    user-select: none;
  }
  .card {
    position: relative;
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 2px;
    justify-items: center;
    max-width: 100%;
    text-align: center;
  }
  .mark {
    font-size: 1.75rem;
    /* Room for the whole glyph: an emoji is taller than its font size. */
    line-height: 1.25;
  }
  h1 {
    /* Two lines, then an ellipsis: a name can be forty characters. */
    display: -webkit-box;
    max-width: 100%;
    margin: 0;
    overflow: hidden;
    font-size: var(--fs-caption);
    font-weight: var(--fw-display);
    line-height: 1.2;
    overflow-wrap: anywhere;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
  }
  p {
    margin: 0;
    color: var(--ink-muted);
    font-size: var(--fs-eyebrow);
    font-weight: var(--fw-emphasis);
    line-height: 1.3;
  }

  /* The words of a call to action, in its colour. Not a button: the tap is
   * Discord's, and lands on the tile wherever it falls. */
  .cue {
    padding: 1px var(--space-xs);
    color: var(--inverse-ink);
    background: var(--inverse-canvas);
    border-radius: var(--radius-pill);
  }
  /* One line more than the smallest tile has room for under a two-line
   * name: the mark gives up the difference. */
  .cued .mark {
    font-size: 1.25rem;
  }

  /* Over a picture the words get a ground of their own along the bottom, so
   * they read on any photo and most of the photo still shows. */
  .pictured {
    place-items: end stretch;
    padding: calc(var(--space-xxs) + var(--cut-y)) calc(var(--space-xxs) + var(--cut-x));
  }
  .pictured .card {
    grid-template-columns: auto minmax(0, 1fr);
    grid-template-areas: 'name name' 'mark detail' 'cue cue';
    gap: 0 var(--space-xxs);
    justify-items: start;
    padding: var(--space-xxs) var(--space-xs);
    text-align: left;
    background: var(--surface-1);
    border-radius: var(--radius-sm);
  }
  .pictured .mark {
    grid-area: mark;
    font-size: var(--fs-eyebrow);
    line-height: 1.3;
  }
  .pictured h1 {
    grid-area: name;
  }
  .pictured p {
    grid-area: detail;
  }
  .pictured .cue {
    grid-area: cue;
    margin-top: 2px;
  }
</style>
