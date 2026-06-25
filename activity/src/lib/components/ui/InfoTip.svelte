<script module lang="ts">
  let nextId = 0;
</script>

<script lang="ts">
  // A small "?" affordance that reveals an explanation on hover or keyboard
  // focus. Accessible: it's a real button, the bubble is its aria-description.
  interface Props {
    text: string;
    /** What the tip explains, for the button's accessible name. */
    label?: string;
  }
  let { text, label }: Props = $props();

  nextId += 1;
  const id = `infotip-${nextId}`;
</script>

<span class="info">
  <button
    type="button"
    class="dot"
    aria-label={label ? `Help: ${label}` : 'More information'}
    aria-describedby={id}
  >
    ?
  </button>
  <span {id} role="tooltip" class="bubble">{text}</span>
</span>

<style>
  .info {
    position: relative;
    display: inline-flex;
    vertical-align: middle;
  }
  .dot {
    display: grid;
    place-items: center;
    width: 16px;
    height: 16px;
    padding: 0;
    color: var(--ink-muted);
    font-size: 0.6875rem;
    font-weight: var(--fw-emphasis);
    line-height: 1;
    background: var(--surface-3);
    border: 0;
    border-radius: var(--radius-pill);
    cursor: help;
  }
  .dot:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .bubble {
    position: absolute;
    bottom: calc(100% + 6px);
    left: 50%;
    z-index: 5;
    width: max-content;
    max-width: 240px;
    padding: var(--space-xs) var(--space-sm);
    color: var(--ink);
    font-size: var(--fs-caption);
    font-weight: var(--fw-body);
    line-height: 1.4;
    letter-spacing: 0;
    text-transform: none;
    white-space: normal;
    background: var(--surface-1);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-md);
    box-shadow: var(--shadow-medium);
    opacity: 0;
    visibility: hidden;
    transform: translateX(-50%);
    transition: opacity var(--motion-fast) var(--ease);
    pointer-events: none;
  }
  .dot:hover + .bubble,
  .dot:focus-visible + .bubble {
    opacity: 1;
    visibility: visible;
  }
</style>
