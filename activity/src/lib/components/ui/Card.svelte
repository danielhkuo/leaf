<script lang="ts">
  // Outlined, mostly-flat container (DESIGN.md §6: outlines before shadows).
  import type { Snippet } from 'svelte';

  interface Props {
    /** Adds a left accent bar (the per-series identity color). */
    accent?: boolean;
    /** Removes inner padding (e.g. media-edge cards). */
    flush?: boolean;
    label?: string;
    children: Snippet;
  }
  let { accent = false, flush = false, label, children }: Props = $props();
</script>

<section class="card" class:accent class:flush aria-label={label}>
  {@render children()}
</section>

<style>
  .card {
    background: var(--surface-1);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-xl);
    padding: var(--space-md);
  }
  .card.flush {
    padding: 0;
    overflow: hidden;
  }
  .card.accent {
    border-left: 2px solid var(--accent);
  }
</style>
