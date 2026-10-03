<script lang="ts">
  // Mirrors App.svelte's boot screens (markup and styles) for one fixed
  // state: the real shell starts the Discord handshake as it mounts, which a
  // browser tab cannot answer. Every word comes from the functions App uses
  // (bootStatusText, bootErrorCopy), so the mock can only show a state the
  // boot can produce.
  import Button from '../lib/components/ui/Button.svelte';
  import { bootErrorCopy } from '../lib/sdk/bootError';
  import { bootStatusText, type SessionState } from '../lib/stores/session.svelte';

  interface Props {
    /** The boot state to show, as the session store holds it. */
    boot: Exclude<SessionState, { status: 'authed' }>;
  }
  let { boot }: Props = $props();

  const failure = $derived(boot.status === 'error' ? boot.error : null);
  const copy = $derived(failure ? bootErrorCopy(failure.kind) : null);
  const stuck = $derived(boot.status === 'loading' && boot.slow && boot.step === 'authorize');
  const noop = (): void => undefined;
</script>

<main class="boot">
  {#if boot.status === 'loading'}
    <div class="center">
      <div class="center" role="status" aria-live="polite">
        <span class="mark sway" aria-hidden="true">🍃</span>
        <p class="status">{bootStatusText(boot.step, boot.slow)}</p>
        {#if stuck}
          <p class="hint">If Discord isn’t asking, close leaf and open it again.</p>
        {/if}
      </div>
      {#if stuck}
        <div class="actions">
          <Button variant="secondary" onclick={noop}>Close leaf</Button>
        </div>
      {/if}
    </div>
  {:else if failure && copy}
    <div class="center" role={copy.tone === 'error' ? 'alert' : 'status'}>
      <span class="mark" aria-hidden="true">{copy.tone === 'error' ? '🍂' : '🍃'}</span>
      <h1>{copy.title}</h1>
      <p class="message">{copy.message}</p>
      {#if copy.retry || copy.close}
        <div class="actions">
          {#if copy.retry}
            <Button variant="primary" onclick={noop}>{copy.retry}</Button>
          {/if}
          {#if copy.close}
            <Button variant={copy.retry ? 'secondary' : 'primary'} onclick={noop}>
              Close leaf
            </Button>
          {/if}
        </div>
      {/if}
      {#if copy.details}
        <details class="details">
          <summary>Details</summary>
          <p>{failure.detail}</p>
        </details>
      {/if}
    </div>
  {/if}
</main>

<style>
  .boot {
    display: grid;
    place-items: center;
    min-height: 100%;
    padding: var(--space-lg);
  }
  .center {
    display: grid;
    gap: var(--space-sm);
    justify-items: center;
    max-width: 28rem;
    text-align: center;
  }
  .mark {
    display: inline-block;
    font-size: var(--fs-display);
    line-height: 1;
  }
  .sway {
    transform-origin: 50% 90%;
    animation: sway 2.4s ease-in-out infinite alternate;
  }
  @keyframes sway {
    from {
      transform: rotate(-7deg);
    }
    to {
      transform: rotate(7deg);
    }
  }
  @keyframes breathe {
    to {
      opacity: 0.45;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .sway {
      animation: breathe 1.2s ease-in-out infinite alternate !important;
    }
  }
  .status,
  .message {
    margin: 0;
    color: var(--ink-muted);
  }
  h1 {
    margin: 0;
    font-size: var(--fs-card-title);
    font-weight: var(--fw-display);
    line-height: 1.2;
    letter-spacing: var(--tracking-display);
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-xs);
    justify-content: center;
    margin-top: var(--space-sm);
  }
  .hint {
    margin: 0;
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
  }
  .details {
    color: var(--ink-subtle);
    font-size: var(--fs-caption);
  }
  .details summary {
    padding: 0 var(--space-sm);
    line-height: var(--touch-target);
    cursor: pointer;
  }
  .details p {
    margin: 0;
    font-family: var(--font-mono);
    overflow-wrap: anywhere;
    -webkit-user-select: text;
    user-select: text;
  }
</style>
