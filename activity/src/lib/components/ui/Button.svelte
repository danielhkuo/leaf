<script lang="ts">
  // Unified pill button. `primary` is the single blue call-to-action;
  // `secondary` is outlined; `ghost` is bare; `danger` is the outlined
  // destructive action (revoke, delete). `size="sm"` is the compact 44px form
  // for tight rows such as the viewer's actions.
  import type { Snippet } from 'svelte';

  interface Props {
    variant?: 'primary' | 'secondary' | 'ghost' | 'danger';
    size?: 'md' | 'sm';
    type?: 'button' | 'submit';
    disabled?: boolean;
    full?: boolean;
    ariaLabel?: string;
    onclick?: (e: MouseEvent) => void;
    children: Snippet;
  }
  let {
    variant = 'secondary',
    size = 'md',
    type = 'button',
    disabled = false,
    full = false,
    ariaLabel,
    onclick,
    children,
  }: Props = $props();
</script>

<button
  class="btn {variant}"
  class:sm={size === 'sm'}
  class:full
  {type}
  {disabled}
  {onclick}
  aria-label={ariaLabel}
>
  {@render children()}
</button>

<style>
  .btn {
    display: inline-flex;
    gap: var(--space-xs);
    align-items: center;
    justify-content: center;
    min-height: var(--control-height);
    padding: 0 26px;
    color: var(--ink);
    font: inherit;
    font-size: var(--fs-body);
    font-weight: var(--fw-display);
    white-space: nowrap;
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--radius-pill);
    cursor: pointer;
    transition:
      transform var(--motion-fast) var(--ease),
      opacity var(--motion-base) var(--ease),
      background var(--motion-fast) var(--ease);
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
  .btn.sm {
    min-height: var(--touch-target);
    padding: 0 var(--space-md);
    font-size: var(--fs-body-sm);
  }

  .primary {
    color: var(--inverse-ink);
    background: var(--inverse-canvas);
    border-color: var(--inverse-canvas);
    box-shadow: var(--shadow-soft);
  }
  .secondary {
    border-color: var(--border-strong);
  }
  .ghost {
    color: var(--ink-muted);
  }
  .danger {
    color: var(--error);
    border-color: var(--error);
  }

  /* Touch feedback comes from :active. Hover is for real pointers only;
   * on a phone it would stick to the last tapped button. */
  @media (hover: hover) {
    .primary:hover:not(:disabled) {
      filter: brightness(1.04);
    }
    .secondary:hover:not(:disabled) {
      background: var(--surface-3);
    }
    .ghost:hover:not(:disabled) {
      color: var(--ink);
      background: var(--surface-2);
    }
    .danger:hover:not(:disabled) {
      background: color-mix(in srgb, var(--error-fill) 6%, transparent);
    }
  }
</style>
