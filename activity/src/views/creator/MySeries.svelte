<script lang="ts">
  // The creator's own series, each opening its settings. Reached from the
  // picker's "Manage my series" link.
  import MySeriesCard from '../../lib/components/creator/MySeriesCard.svelte';
  import Callout from '../../lib/components/shared/Callout.svelte';
  import Skeleton from '../../lib/components/shared/Skeleton.svelte';
  import IconButton from '../../lib/components/ui/IconButton.svelte';
  import { getApi, getGuildId } from '../../lib/stores/gallery.svelte';
  import { nav } from '../../lib/stores/nav.svelte';
  import type { MySeries } from '../../lib/types/api';

  let series = $state<MySeries[] | null>(null);
  let failed = $state(false);

  $effect(() => {
    const api = getApi();
    const gid = getGuildId();
    let cancelled = false;
    api
      .listMySeries(gid)
      .then((rows) => {
        if (!cancelled) series = rows;
      })
      .catch(() => {
        if (!cancelled) failed = true;
      });
    return () => {
      cancelled = true;
    };
  });

  function open(s: MySeries): void {
    nav.push({ name: 'seriesSettings', seriesId: s.id });
  }
</script>

<div class="view">
  <header class="bar">
    <IconButton ariaLabel="Back" variant="solid" onclick={() => nav.back()}>←</IconButton>
    <h1>My series</h1>
  </header>

  {#if failed}
    <Callout title="Couldn’t load your series">Try reopening the gallery.</Callout>
  {:else if !series}
    <Skeleton height="200px" radius="var(--radius-xl)" />
  {:else if series.length === 0}
    <Callout title="You haven’t started any series">
      Use “Start a series” to plant your first one.
    </Callout>
  {:else}
    <ul class="list">
      {#each series as s (s.id)}
        <li><MySeriesCard series={s} onOpen={open} /></li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .view {
    display: grid;
    gap: var(--space-md);
    width: 100%;
    max-width: 40rem;
    margin: 0 auto;
    padding: var(--space-md);
  }
  .bar {
    display: flex;
    gap: var(--space-sm);
    align-items: center;
    min-height: var(--appbar-h);
  }
  h1 {
    margin: 0;
    font-size: var(--fs-card-title);
    font-weight: var(--fw-display);
    letter-spacing: var(--tracking-display);
  }
  .list {
    display: grid;
    gap: var(--space-sm);
    margin: 0;
    padding: 0;
    list-style: none;
  }
</style>
