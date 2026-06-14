<script lang="ts">
  import { onMount } from 'svelte';

  import { bootSession, session } from './lib/stores/session.svelte';
  import Gallery from './views/Gallery.svelte';

  onMount(() => {
    void bootSession();
  });
</script>

{#if session.value.status === 'authed'}
  <Gallery session={session.value.session} />
{:else}
  <main class="boot">
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
    {/if}
  </main>
{/if}

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
    text-align: center;
  }
  .leaf {
    font-size: var(--fs-display);
    line-height: 1;
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
    color: var(--on-accent-dark);
    font: inherit;
    font-size: var(--fs-body-sm);
    font-weight: var(--fw-emphasis);
    background: var(--accent);
    border: 0;
    border-radius: var(--radius-md);
    cursor: pointer;
  }
  .btn:active {
    transform: translateY(1px);
  }
</style>
