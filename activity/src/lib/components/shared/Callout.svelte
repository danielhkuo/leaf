<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    title?: string;
    children: Snippet;
  }
  let { title, children }: Props = $props();
</script>

<div class="callout">
  {#if title}<p class="title">{title}</p>{/if}
  <div class="body">{@render children()}</div>
</div>

<style>
  .callout {
    max-width: 32rem;
    padding: var(--space-lg);
    background: var(--surface-1);
    border: 1px solid var(--hairline);
    border-left: 2px solid var(--accent);
    border-radius: var(--radius-xl);
    color: var(--ink-muted);
  }
  .title {
    margin: 0 0 var(--space-xs);
    color: var(--ink);
    font-weight: var(--fw-emphasis);
  }
  .body {
    font-size: var(--fs-body-sm);
  }
</style>
