<script lang="ts">
  import { onMount } from 'svelte';

  import SeriesPicker from '../lib/components/picker/SeriesPicker.svelte';
  import { gallery, rememberSeries } from '../lib/stores/gallery.svelte';
  import { focusHeading, isNavigable, nav } from '../lib/stores/nav.svelte';
  import type { Series } from '../lib/types/api';

  interface Props {
    /** Current viewer, to decide whether they own any series shown here. */
    userId: string;
    /** The channel leaf was opened in; its series are listed first. */
    channelId?: string | null;
  }
  let { userId, channelId = null }: Props = $props();

  // `owns_any` counts revoked series too, which the visible list may not
  // show; the list is the fallback for an older server.
  const ownsSeries = $derived(
    gallery.eligibility?.owns_any ??
      gallery.series.some((s) => s.is_owner ?? s.creator_id === userId),
  );
  const inChannel = $derived.by(() => {
    const here = channelId;
    if (here === null) return [];
    return gallery.series.filter((s) => s.channel_ids?.includes(here)).map((s) => s.id);
  });

  function select(series: Series): void {
    if (!isNavigable(series)) return;
    rememberSeries(series.id);
    nav.push({ name: 'home', seriesId: series.id });
  }

  let root = $state<HTMLElement>();
  onMount(() => focusHeading(root));
</script>

<main bind:this={root}>
  <SeriesPicker
    series={gallery.series}
    onSelect={select}
    eligibility={gallery.eligibility}
    eligibilityStatus={gallery.eligibilityStatus}
    {ownsSeries}
    {inChannel}
    onCreate={() => nav.push({ name: 'createSeries' })}
    onManage={() => nav.push({ name: 'mySeries' })}
  />
</main>
