<script module lang="ts">
  /** The glyphs leaf draws itself. */
  export type IconName = 'back' | 'close' | 'gear' | 'plus' | 'refresh';

  /** Stroke paths on a 24px grid; the gear is drawn from two rings instead. */
  const PATHS = {
    back: 'M19 12H5m7-7-7 7 7 7',
    close: 'M6 6l12 12M18 6 6 18',
    plus: 'M12 5v14M5 12h14',
    // A clockwise circle open at the top right, its arrowhead a corner.
    refresh: 'M20 12a8 8 0 1 1-8-8c2.2 0 4.2.9 5.7 2.3L20 8.5M20 3.5v5h-5',
  } as const;
</script>

<script lang="ts">
  // Inline SVG for the glyphs the bundled Latin font subsets lack: ←, ✕ and ⚙
  // fall back to mismatched system fonts, and the gear can render as a colour
  // emoji that ignores `color`. Decorative: the control around it carries the
  // name. Sized by the surrounding font-size (1em), coloured by `color`.
  interface Props {
    name: IconName;
  }
  let { name }: Props = $props();
</script>

<svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
  {#if name === 'gear'}
    <circle cx="12" cy="12" r="5.75" stroke-width="2.5" />
    <circle cx="12" cy="12" r="8.5" class="teeth" transform="rotate(-13.14 12 12)" />
  {:else}
    <path d={PATHS[name]} />
  {/if}
</svg>

<style>
  svg {
    display: block;
    flex: none;
    width: 1em;
    height: 1em;
    fill: none;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  /* Gear teeth: a thick ring dashed into eight blocks (r 8.5, so 53.41 round;
   * the rotation on the circle centres a tooth on each axis). */
  .teeth {
    stroke-width: 3;
    stroke-dasharray: 3.9 2.776;
    stroke-linecap: butt;
  }
</style>
