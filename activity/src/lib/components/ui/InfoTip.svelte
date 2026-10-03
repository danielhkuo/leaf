<script module lang="ts">
  let nextId = 0;

  /** Space kept clear between the bubble and the viewport edge, in px. */
  const GUTTER = 8;
  /** Space between the dot and the bubble, in px (mirrored in the CSS). */
  const GAP = 6;

  /**
   * How far to slide a bubble sideways (px, positive = right) so it stays
   * inside the viewport. The bubble is centred on `centre` by default; one
   * wider than the viewport is pinned to the left gutter.
   */
  export function bubbleShift(
    centre: number,
    width: number,
    viewport: number,
    gutter: number = GUTTER,
  ): number {
    const ideal = centre - width / 2;
    const max = Math.max(gutter, viewport - gutter - width);
    return Math.min(Math.max(ideal, gutter), max) - ideal;
  }
</script>

<script lang="ts">
  // A small "?" that opens an explanation when tapped or clicked — a
  // disclosure, not a hover tooltip, so it works on touch and stays open until
  // dismissed (tap the bubble or anywhere outside, Escape, or tab away). The
  // bubble sits in a live region so screen readers read it when it opens.
  interface Props {
    text: string;
    /** What the tip explains, for the button's accessible name. */
    label?: string;
  }
  let { text, label }: Props = $props();

  nextId += 1;
  const id = `infotip-${nextId}`;

  let open = $state(false);
  let shift = $state(0);
  let below = $state(false);
  let root = $state<HTMLElement>();
  let dot = $state<HTMLButtonElement>();
  let bubble = $state<HTMLElement>();

  $effect(() => {
    if (!open || !root || !dot || !bubble) return;
    const rootEl = root;
    const dotEl = dot;
    const bubbleEl = bubble;

    const place = (): void => {
      const rect = dotEl.getBoundingClientRect();
      shift = bubbleShift(
        rect.left + rect.width / 2,
        bubbleEl.offsetWidth,
        document.documentElement.clientWidth,
      );
      // Flip under the dot when there is no room above it.
      below = rect.top - bubbleEl.offsetHeight - GAP < GUTTER;
    };
    const onPointerDown = (e: PointerEvent): void => {
      if (e.target instanceof Node && !rootEl.contains(e.target)) open = false;
    };
    const onKeyDown = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return;
      // Only the tip closes; a dialog around it keeps its own Escape.
      e.stopPropagation();
      open = false;
      dotEl.focus();
    };
    // A tap on the bubble dismisses the tip and goes no further: it must not
    // reach the control the bubble covers, a <label> the tip sits in, or a
    // click handler further up.
    const onBubbleClick = (e: MouseEvent): void => {
      e.preventDefault();
      e.stopPropagation();
      open = false;
    };
    // Keep focus where it is while the bubble is pressed. Otherwise the press
    // blurs the button, focusout removes the bubble, and the rest of the tap
    // lands on whatever was underneath.
    const onBubbleMouseDown = (e: MouseEvent): void => {
      e.preventDefault();
    };

    place();
    document.addEventListener('pointerdown', onPointerDown, true);
    document.addEventListener('keydown', onKeyDown, true);
    window.addEventListener('resize', place);
    bubbleEl.addEventListener('click', onBubbleClick);
    bubbleEl.addEventListener('mousedown', onBubbleMouseDown);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown, true);
      document.removeEventListener('keydown', onKeyDown, true);
      window.removeEventListener('resize', place);
      bubbleEl.removeEventListener('click', onBubbleClick);
      bubbleEl.removeEventListener('mousedown', onBubbleMouseDown);
    };
  });

  function onFocusOut(e: FocusEvent): void {
    if (!(e.relatedTarget instanceof Node) || !root?.contains(e.relatedTarget)) open = false;
  }
</script>

<span class="info" bind:this={root} onfocusout={onFocusOut}>
  <button
    type="button"
    class="dot"
    bind:this={dot}
    aria-label={label ? `Help: ${label}` : 'More information'}
    aria-expanded={open}
    aria-controls={id}
    onclick={() => (open = !open)}
  >
    ?
  </button>
  <span {id} role="status">
    {#if open}
      <span class="bubble" class:below bind:this={bubble} style:--shift="{shift}px">{text}</span>
    {/if}
  </span>
</span>

<style>
  .info {
    position: relative;
    display: inline-flex;
    vertical-align: middle;
  }
  .dot {
    position: relative;
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    padding: 0;
    color: var(--ink-muted);
    font: inherit;
    font-size: var(--fs-eyebrow);
    font-weight: var(--fw-display);
    line-height: 1;
    background: var(--surface-3);
    border: 0;
    border-radius: var(--radius-pill);
    cursor: pointer;
  }
  /* A 44x40px hit area around the 20px glyph, without moving the layout. It
   * reaches up rather than down: the tip usually ends a label row whose input
   * starts 4px below, and a positioned hit area that overlapped the input
   * would take the taps along its top edge. */
  .dot::after {
    content: '';
    position: absolute;
    inset: -16px -12px -4px;
  }
  .dot[aria-expanded='true'] {
    color: var(--surface-1);
    background: var(--ink);
  }
  .bubble {
    position: absolute;
    bottom: calc(100% + 6px);
    left: 50%;
    z-index: 5;
    width: max-content;
    max-width: min(240px, calc(100vw - 32px));
    padding: var(--space-xs) var(--space-sm);
    color: var(--ink);
    font-size: var(--fs-caption);
    font-weight: var(--fw-body);
    line-height: 1.4;
    letter-spacing: 0;
    text-align: left;
    text-transform: none;
    white-space: normal;
    background: var(--surface-1);
    border: 1px solid var(--control-border);
    border-radius: var(--radius-md);
    box-shadow: var(--shadow-medium);
    transform: translateX(calc(-50% + var(--shift, 0px)));
  }
  .bubble.below {
    top: calc(100% + 6px);
    bottom: auto;
  }
</style>
