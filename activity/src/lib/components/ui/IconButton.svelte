<script lang="ts">
  // Square icon control: nav arrows, close, zoom. `ghost` sits on a solid
  // surface; `overlay` sits over imagery (translucent ground for contrast).
  // Pass `icon` for the glyphs the bundled fonts lack (see Icon.svelte);
  // children still work for glyphs the fonts do carry (‹ › + −).
  import type { Snippet } from 'svelte';

  import Icon, { type IconName } from './Icon.svelte';

  interface Props {
    ariaLabel: string;
    variant?: 'ghost' | 'solid' | 'overlay';
    icon?: IconName;
    disabled?: boolean;
    onclick?: (e: MouseEvent) => void;
    children?: Snippet;
  }
  let { ariaLabel, variant = 'ghost', icon, disabled = false, onclick, children }: Props = $props();
</script>

<button type="button" class="icon {variant}" {disabled} {onclick} aria-label={ariaLabel}>
  {#if icon}
    <Icon name={icon} />
  {:else}
    {@render children?.()}
  {/if}
</button>

<style>
  .icon {
    display: grid;
    flex: none; /* never squeezed below the touch target in a crowded bar */
    place-items: center;
    width: var(--touch-target);
    height: var(--touch-target);
    padding: 0;
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
  .icon:active {
    transform: scale(0.94);
  }
  .icon:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .solid {
    background: var(--surface-2);
    border-color: var(--control-border);
  }
  .overlay {
    color: #ffffff;
    background: rgb(0 0 0 / 50%);
    border-color: var(--hairline);
  }

  @media (hover: hover) {
    .ghost:hover:not(:disabled) {
      background: var(--surface-2);
    }
    .solid:hover:not(:disabled) {
      background: var(--surface-3);
    }
    .overlay:hover:not(:disabled) {
      background: rgb(0 0 0 / 68%);
    }
  }
</style>
