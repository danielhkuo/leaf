<script lang="ts">
  // A bordered note: empty states, policy explanations, banners, failures.
  // `tone` colours the edge; only `error` is announced (role="alert"), so an
  // empty state never interrupts a screen reader. `action` holds the buttons
  // that resolve it (Try again, Back). `heading` makes the title the page's
  // <h1>, for a note that is all the screen shows; it looks the same.
  import type { Snippet } from 'svelte';

  interface Props {
    title?: string | undefined;
    heading?: boolean;
    tone?: 'neutral' | 'warning' | 'error';
    action?: Snippet | undefined;
    children?: Snippet | undefined;
  }
  let { title, heading = false, tone = 'neutral', action, children }: Props = $props();
</script>

<div class="callout {tone}" role={tone === 'error' ? 'alert' : undefined}>
  {#if title && heading}
    <h1 class="title">{title}</h1>
  {:else if title}
    <p class="title">{title}</p>
  {/if}
  {#if children}<div class="body">{@render children()}</div>{/if}
  {#if action}<div class="actions">{@render action()}</div>{/if}
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
  .warning {
    border-left-color: var(--warning-fill);
  }
  .error {
    border-left-color: var(--error-fill);
  }
  .title {
    margin: 0;
    color: var(--ink);
    font: inherit;
    font-weight: var(--fw-emphasis);
  }
  .body {
    font-size: var(--fs-body-sm);
  }
  .title + .body {
    margin-top: var(--space-xs);
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-xs);
    margin-top: var(--space-md);
  }
</style>
