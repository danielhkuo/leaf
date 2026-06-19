<script lang="ts">
  // Square icon control: nav arrows, close, zoom. `ghost` sits on a solid
  // surface; `overlay` sits over imagery (translucent ground for contrast).
  import type { Snippet } from 'svelte';

  interface Props {
    ariaLabel: string;
    variant?: 'ghost' | 'solid' | 'overlay';
    disabled?: boolean;
    onclick?: (e: MouseEvent) => void;
    children: Snippet;
  }
  let { ariaLabel, variant = 'ghost', disabled = false, onclick, children }: Props = $props();
</script>

<button class="icon {variant}" {disabled} {onclick} aria-label={ariaLabel}>
  {@render children()}
</button>

<style>
  .icon {
    display: grid;
    place-items: center;
    width: var(--touch-target);
    height: var(--touch-target);
    color: var(--ink);
    font: inherit;
    font-size: 1.25rem;
    line-height: 1;
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--radius-pill);
    cursor: pointer;
    transition:
      transform var(--motion-fast) var(--ease),
      background var(--motion-fast) var(--ease);
  }
  .icon:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .icon:active {
    transform: scale(0.94);
  }
  .icon:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .ghost:hover {
    background: var(--surface-2);
  }
  .solid {
    background: var(--surface-2);
    border-color: var(--hairline);
  }
  .solid:hover {
    background: var(--surface-3);
  }
  .overlay {
    color: #ffffff;
    background: rgb(0 0 0 / 50%);
    border-color: var(--hairline);
  }
  .overlay:hover {
    background: rgb(0 0 0 / 68%);
  }
</style>
