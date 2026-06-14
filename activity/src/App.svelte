<script lang="ts">
  import { onMount } from 'svelte';

  import { displayName } from './lib/sdk/handshake';
  import { bootSession, session } from './lib/stores/session.svelte';

  onMount(() => {
    void bootSession();
  });
</script>

<main class="shell">
  {#if session.value.status === 'loading'}
    <div class="center" role="status" aria-live="polite">
      <span class="leaf" aria-hidden="true">🍃</span>
      <p class="muted">Connecting to Discord…</p>
    </div>
  {:else if session.value.status === 'error'}
    <div class="center" role="alert">
      <span class="leaf" aria-hidden="true">🍂</span>
      <p>Couldn’t start the gallery.</p>
      <p class="muted small">{session.value.error}</p>
      <button class="btn" onclick={() => void bootSession()}>Try again</button>
    </div>
  {:else if session.value.status === 'authed'}
    <div class="center">
      <span class="leaf" aria-hidden="true">🍃</span>
      <h1>Hello, {displayName(session.value.session.user)}</h1>
      <p class="muted small">
        guild {session.value.session.guildId ?? 'unknown'} · the gallery arrives next phase
      </p>
    </div>
  {/if}
</main>

<style>
  .shell {
    min-height: 100%;
    display: grid;
    place-items: center;
    padding: var(--space-lg);
  }
  .center {
    display: grid;
    gap: var(--space-sm);
    justify-items: center;
    text-align: center;
  }
  .leaf {
    font-size: var(--fs-display);
    line-height: 1;
  }
  h1 {
    margin: 0;
    font-size: var(--fs-headline);
    font-weight: var(--fw-emphasis);
  }
  .muted {
    margin: 0;
    color: var(--ink-muted);
  }
  .small {
    font-size: var(--fs-body-sm);
  }
  .btn {
    margin-top: var(--space-xs);
    padding: 10px 18px;
    font: inherit;
    font-size: var(--fs-body-sm);
    font-weight: var(--fw-emphasis);
    color: var(--on-accent-dark);
    background: var(--accent);
    border: 0;
    border-radius: var(--radius-md);
    cursor: pointer;
  }
  .btn:active {
    transform: translateY(1px);
  }
</style>
