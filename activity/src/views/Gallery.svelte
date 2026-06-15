<script lang="ts">
  import { onMount } from 'svelte';

  import Callout from '../lib/components/shared/Callout.svelte';
  import Skeleton from '../lib/components/shared/Skeleton.svelte';
  import type { Session } from '../lib/sdk/handshake';
  import { gallery, initGallery, lastSeries } from '../lib/stores/gallery.svelte';
  import { nav } from '../lib/stores/nav.svelte';
  import Home from './Home.svelte';
  import Picker from './Picker.svelte';
  import Viewer from './Viewer.svelte';

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
    // Seed the picker beneath any remembered series so the back button always
    // returns to the full series list (rather than stranding you in one).
    const remembered = lastSeries();
    nav.reset({ name: 'picker' });
    if (remembered !== null && list.some((s) => s.id === remembered)) {
      nav.push({ name: 'home', seriesId: remembered });
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
    <Viewer series={activeSeries} day={view.day} onClose={() => nav.back()} />
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
</style>
