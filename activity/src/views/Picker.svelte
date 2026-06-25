<script lang="ts">
  import SeriesPicker from '../lib/components/picker/SeriesPicker.svelte';
  import { gallery, rememberSeries } from '../lib/stores/gallery.svelte';
  import { nav } from '../lib/stores/nav.svelte';
  import type { Series } from '../lib/types/api';

  interface Props {
    /** Current viewer, to decide whether they own any series shown here. */
    userId: string;
  }
  let { userId }: Props = $props();

  const ownsSeries = $derived(gallery.series.some((s) => s.creator_id === userId));

  function select(series: Series): void {
    rememberSeries(series.id);
    nav.push({ name: 'home', seriesId: series.id });
  }
</script>

<SeriesPicker
  series={gallery.series}
  onSelect={select}
  eligibility={gallery.eligibility}
  eligibilityStatus={gallery.eligibilityStatus}
  {ownsSeries}
  onCreate={() => nav.push({ name: 'createSeries' })}
  onManage={() => nav.push({ name: 'mySeries' })}
/>
