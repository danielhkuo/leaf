<script lang="ts">
  import { onMount } from 'svelte';

  import Callout from '../lib/components/shared/Callout.svelte';
  import Skeleton from '../lib/components/shared/Skeleton.svelte';
  import type { Session } from '../lib/sdk/handshake';
  import { gallery, initGallery, lastSeries } from '../lib/stores/gallery.svelte';
  import { nav, type View } from '../lib/stores/nav.svelte';
  import type { CreateSeries, MySeries, SeriesSettings } from './creator/entry';
  import Home from './Home.svelte';
  import Picker from './Picker.svelte';
  import Viewer from './Viewer.svelte';

  interface Props {
    session: Session;
  }
  let { session }: Props = $props();

  const userId = $derived(session.user.id);

  // Creator views are a separate chunk, imported on first navigation so they
  // cost gallery-only users nothing (PERF.md budget). `null` until loaded.
  interface CreatorViews {
    CreateSeries: typeof CreateSeries;
    MySeries: typeof MySeries;
    SeriesSettings: typeof SeriesSettings;
  }
  let creator = $state<CreatorViews | null>(null);

  function isCreatorView(name: View['name']): boolean {
    return name === 'createSeries' || name === 'mySeries' || name === 'seriesSettings';
  }

  $effect(() => {
    if (isCreatorView(view.name) && !creator) {
      void import('./creator/entry').then((m) => {
        creator = m;
      });
    }
  });

  onMount(() => {
    void start();
  });

  async function start(): Promise<void> {
    await initGallery(session);
    if (gallery.status !== 'ready') return;
    const list = gallery.series;
    const [only] = list;
    if (list.length === 1 && only) {
      // Keep the picker beneath home so Back reaches "Start a series".
      nav.reset({ name: 'picker' });
      nav.push({ name: 'home', seriesId: only.id });
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
  <Picker {userId} />
{:else if isCreatorView(view.name)}
  {#if creator}
    {#if view.name === 'createSeries'}
      <creator.CreateSeries />
    {:else if view.name === 'mySeries'}
      <creator.MySeries />
    {:else if view.name === 'seriesSettings'}
      <creator.SeriesSettings seriesId={view.seriesId} />
    {/if}
  {:else}
    <div class="boot"><Skeleton width="240px" height="20px" /></div>
  {/if}
{:else if activeSeries}
  <Home series={activeSeries} {userId} canGoBack={nav.canGoBack} />
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
