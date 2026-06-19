<script lang="ts">
  // Unified pill button (DESIGN.md §4). `primary` is the inverse (white on
  // black ground) call-to-action; `secondary` is outlined; `ghost` is bare.
  import type { Snippet } from 'svelte';

  interface Props {
    variant?: 'primary' | 'secondary' | 'ghost';
    type?: 'button' | 'submit';
    disabled?: boolean;
    full?: boolean;
    ariaLabel?: string;
    onclick?: (e: MouseEvent) => void;
    children: Snippet;
  }
  let {
    variant = 'secondary',
    type = 'button',
    disabled = false,
    full = false,
    ariaLabel,
    onclick,
    children,
  }: Props = $props();
</script>

<button class="btn {variant}" class:full {type} {disabled} {onclick} aria-label={ariaLabel}>
  {@render children()}
</button>

<style>
  .btn {
    display: inline-flex;
    gap: var(--space-xs);
    align-items: center;
    justify-content: center;
    min-height: var(--control-height);
    padding: 0 22px;
    color: var(--ink);
    font: inherit;
    font-size: var(--fs-body-sm);
    font-weight: var(--fw-emphasis);
    white-space: nowrap;
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--radius-pill);
    cursor: pointer;
    user-select: none;
    transition:
      transform var(--motion-fast) var(--ease),
      opacity var(--motion-base) var(--ease),
      background var(--motion-fast) var(--ease);
  }
  .btn:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .btn:active {
    transform: scale(0.98);
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .btn.full {
    width: 100%;
  }

  .primary {
    color: var(--inverse-ink);
    background: var(--inverse-canvas);
    border-color: var(--inverse-canvas);
  }
  .primary:hover {
    opacity: 0.88;
  }

  .secondary {
    border-color: var(--hairline-strong);
  }
  .secondary:hover {
    background: var(--surface-2);
  }

  .ghost {
    color: var(--ink-muted);
  }
  .ghost:hover {
    color: var(--ink);
    background: var(--surface-2);
  }
</style>
