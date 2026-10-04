<script lang="ts">
  // The one failure state for anything that did not load: what happened, what
  // to do about it, and the buttons that do it. `onRetry` re-runs the load in
  // place; `onBack` leaves the screen. `message` is a human sentence (see
  // describeError) — never raw request or exception text. `page` is for a
  // failure that is the whole screen: its title is then the page's heading.
  import Button from '../ui/Button.svelte';
  import Callout from './Callout.svelte';

  interface Props {
    title: string;
    message?: string | undefined;
    onRetry?: (() => void) | undefined;
    onBack?: (() => void) | undefined;
    retryLabel?: string;
    backLabel?: string;
    page?: boolean;
  }
  let {
    title,
    message,
    onRetry,
    onBack,
    retryLabel = 'Try again',
    backLabel = 'Back',
    page = false,
  }: Props = $props();
</script>

{#snippet body()}{message}{/snippet}

{#snippet actions()}
  {#if onRetry}<Button variant="primary" onclick={onRetry}>{retryLabel}</Button>{/if}
  {#if onBack}<Button variant="secondary" onclick={onBack}>{backLabel}</Button>{/if}
{/snippet}

<Callout
  {title}
  heading={page}
  tone="error"
  children={message ? body : undefined}
  action={onRetry || onBack ? actions : undefined}
/>
