<script lang="ts">
  // The shell: boot progress, boot failures, and the gallery once signed in.
  // It also gives a passing notice for errors nothing else handled (a
  // rejected promise, a throw in an event handler or an effect), which would
  // otherwise leave a dead button or a half-updated view with no explanation.
  import { onMount } from 'svelte';

  import Button from './lib/components/ui/Button.svelte';
  import { closeActivity } from './lib/sdk/actions';
  import { bootErrorCopy } from './lib/sdk/bootError';
  import { preloadGallery } from './lib/stores/gallery.svelte';
  import { bootSession, bootStatusText, session } from './lib/stores/session.svelte';
  import Gallery from './views/Gallery.svelte';

  /** How long Discord gets to act on a close before the hint appears. */
  const CLOSE_HINT_AFTER_MS = 1_500;
  const CLOSE_HINT = 'If leaf is still open, close it with Discord’s own controls.';
  const TOAST_MS = 6_000;
  /**
   * Errors this close to the last notice are not logged again. Logging goes
   * out to Discord too, so an error raised by reporting one must not loop.
   */
  const NOTICE_GAP_MS = 1_000;
  /** Failures that are part of normal use and need no notice. */
  const QUIET_ERRORS = new Set(['AbortError', 'NotAllowedError']);

  const current = $derived(session.value);
  const failure = $derived(current.status === 'error' ? current.error : null);
  const copy = $derived(failure ? bootErrorCopy(failure.kind) : null);
  // `authorize` has no timeout: the permission sheet may be open. If it is
  // not (dismissed, or never shown), this is the way out.
  const stuck = $derived(
    current.status === 'loading' && current.slow && current.step === 'authorize',
  );

  /** Runs the boot, or resumes it at the step that failed. */
  function boot(): void {
    void bootSession({ onToken: preloadGallery });
  }

  // Discord does not confirm a close. If this screen is still up a moment
  // later, say how to leave instead of leaving a dead button. The SDK stops
  // listening once it has asked, so nothing here can work afterwards.
  let closing = $state(false);
  let closeHint = $state(false);
  let closeTimer: ReturnType<typeof setTimeout> | undefined;
  async function close(): Promise<void> {
    closing = await closeActivity();
    clearTimeout(closeTimer);
    closeTimer = setTimeout(() => (closeHint = true), closing ? CLOSE_HINT_AFTER_MS : 0);
  }

  let toast = $state(false);
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  let lastNotice = Number.NEGATIVE_INFINITY;
  function notice(reason: unknown): void {
    const name = (reason as { name?: unknown } | null)?.name;
    if (typeof name === 'string' && QUIET_ERRORS.has(name)) return;
    // The browser logs every uncaught error itself; only the log sent on to
    // Discord is lost for one inside the gap.
    const at = Date.now();
    if (at - lastNotice < NOTICE_GAP_MS) return;
    lastNotice = at;
    console.error('leaf: unhandled error', reason);
    toast = true;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = false), TOAST_MS);
  }

  onMount(() => {
    const onRejection = (e: PromiseRejectionEvent): void => notice(e.reason);
    // Resource load failures do not reach `window`; this is thrown script
    // errors only. Browsers report a benign ResizeObserver loop the same way.
    const onError = (e: ErrorEvent): void => {
      if (!e.message.includes('ResizeObserver')) notice(e.error ?? e.message);
    };
    window.addEventListener('unhandledrejection', onRejection);
    window.addEventListener('error', onError);
    boot();
    return () => {
      window.removeEventListener('unhandledrejection', onRejection);
      window.removeEventListener('error', onError);
      clearTimeout(closeTimer);
      clearTimeout(toastTimer);
    };
  });
</script>

{#if current.status === 'authed'}
  <Gallery session={current.session} />
{:else}
  <main class="boot">
    {#if current.status === 'loading'}
      <div class="center">
        <div class="center" role="status" aria-live="polite">
          <span class="mark sway" aria-hidden="true">🍃</span>
          <p class="status">{bootStatusText(current.step, current.slow)}</p>
          {#if stuck}
            <p class="hint">If Discord isn’t asking, close leaf and open it again.</p>
          {/if}
        </div>
        {#if stuck}
          <div class="actions">
            <Button variant="secondary" disabled={closing} onclick={() => void close()}>
              Close leaf
            </Button>
          </div>
          {#if closeHint}
            <p class="hint">{CLOSE_HINT}</p>
          {/if}
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
              <Button variant="primary" disabled={closing} onclick={boot}>{copy.retry}</Button>
            {/if}
            {#if copy.close}
              <Button
                variant={copy.retry ? 'secondary' : 'primary'}
                disabled={closing}
                onclick={() => void close()}
              >
                Close leaf
              </Button>
            {/if}
          </div>
        {/if}
        {#if closeHint}
          <p class="hint">{CLOSE_HINT}</p>
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
{/if}

<!-- Always in the page, so the notice is announced when its text arrives. -->
<div class="toasts" role="status">
  {#if toast}<p class="toast">Something didn’t work. Try that again.</p>{/if}
</div>

<style>
  /* The loading state matches the static shell in index.html, so the swap
   * from one to the other on mount is not visible. Keep the two in step. */
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
  /* !important because app.css's reduced-motion reset is !important too: it
   * would leave a frozen leaf. A slow fade still reads as "working". */
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
  .toasts {
    position: fixed;
    right: max(var(--space-md), var(--safe-right));
    bottom: max(var(--space-md), var(--safe-bottom));
    left: max(var(--space-md), var(--safe-left));
    z-index: 20;
    display: flex;
    justify-content: center;
    pointer-events: none;
  }
  .toast {
    max-width: 28rem;
    margin: 0;
    padding: var(--space-sm) var(--space-md);
    color: var(--ink);
    font-size: var(--fs-body-sm);
    text-align: center;
    background: var(--surface-1);
    border: 1px solid var(--hairline-strong);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-medium);
  }
</style>
