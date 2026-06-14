<script lang="ts">
  import { onMount } from 'svelte';

  import Callout from '../lib/components/shared/Callout.svelte';
  import Skeleton from '../lib/components/shared/Skeleton.svelte';
  import type { Session } from '../lib/sdk/handshake';
  import { gallery, initGallery, lastSeries } from '../lib/stores/gallery.svelte';
  import { nav } from '../lib/stores/nav.svelte';
  import Home from './Home.svelte';
  import Picker from './Picker.svelte';

  interface Props {
    session: Session;
  }
  let { session }: Props = $props();

  onMount(() => {
    void start();
  });

  async function start(): Promise<void> {
    await initGallery(session);
    if (gallery.status !== 'ready') return;
    const list = gallery.series;
    const [only] = list;
    if (list.length === 1 && only) {
      nav.reset({ name: 'home', seriesId: only.id });
      return;
    }
    const remembered = lastSeries();
    if (remembered !== null && list.some((s) => s.id === remembered)) {
      nav.reset({ name: 'home', seriesId: remembered });
    } else {
      nav.reset({ name: 'picker' });
    }
  }

  const view = $derived(nav.current);
  const activeSeries = $derived(
    view.name === 'home' || view.name === 'viewer'
      ? (gallery.series.find((s) => s.id === view.seriesId) ?? null)
      : null,
  );
</script>

{#if gallery.status === 'loading'}
  <div class="boot"><Skeleton width="240px" height="20px" /></div>
{:else if gallery.status === 'error'}
  <div class="boot"><Callout title="Couldn’t load the gallery">{gallery.error}</Callout></div>
{:else if view.name === 'picker'}
  <Picker />
{:else if activeSeries}
  <Home series={activeSeries} canGoBack={nav.canGoBack} />
  {#if view.name === 'viewer'}
    <div class="viewer-stub" role="dialog" aria-modal="true" aria-label={`Day ${view.day}`}>
      <div class="card">
        <p class="eyebrow">Day {view.day}</p>
        <p class="muted">The day viewer arrives in the next phase.</p>
        <button class="btn" onclick={() => nav.back()}>Close</button>
      </div>
    </div>
  {/if}
{:else}
  <div class="boot">
    <Callout title="Series unavailable">That series isn’t available here.</Callout>
  </div>
{/if}

<style>
  .boot {
    display: grid;
    place-items: center;
    min-height: 60vh;
    padding: var(--space-lg);
  }
  .viewer-stub {
    position: fixed;
    inset: 0;
    display: grid;
    place-items: center;
    padding: var(--space-lg);
    background: rgb(0 0 0 / 80%);
  }
  .card {
    display: grid;
    gap: var(--space-sm);
    justify-items: center;
    padding: var(--space-xl);
    text-align: center;
    background: var(--surface-1);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-lg);
  }
  .eyebrow {
    margin: 0;
    color: var(--ink-subtle);
    font-size: var(--fs-eyebrow);
    font-weight: var(--fw-emphasis);
    letter-spacing: 0.6px;
    text-transform: uppercase;
  }
  .muted {
    margin: 0;
    color: var(--ink-muted);
  }
  .btn {
    padding: 10px 18px;
    color: var(--on-accent-dark);
    font: inherit;
    font-size: var(--fs-body-sm);
    font-weight: var(--fw-emphasis);
    background: var(--brand);
    border: 0;
    border-radius: var(--radius-md);
    cursor: pointer;
  }
</style>
