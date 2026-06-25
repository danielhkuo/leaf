<script lang="ts">
  import SeriesPicker from '../lib/components/picker/SeriesPicker.svelte';
  import { gallery, getApi, getGuildId, rememberSeries } from '../lib/stores/gallery.svelte';
  import { nav } from '../lib/stores/nav.svelte';
  import type { Eligibility, Series } from '../lib/types/api';

  interface Props {
    /** Current viewer, to decide whether they own any series shown here. */
    userId: string;
  }
  let { userId }: Props = $props();

  // Eligibility drives the "start a series" entry point. Best-effort: if it
  // fails we simply omit the CTA rather than blocking the picker.
  let eligibility = $state<Eligibility | null>(null);

  $effect(() => {
    let cancelled = false;
    getApi()
      .getEligibility(getGuildId())
      .then((e) => {
        if (!cancelled) eligibility = e;
      })
      .catch(() => {
        /* leave eligibility null — no CTA, picker still works */
      });
    return () => {
      cancelled = true;
    };
  });

  const ownsSeries = $derived(gallery.series.some((s) => s.creator_id === userId));

  function select(series: Series): void {
    rememberSeries(series.id);
    nav.push({ name: 'home', seriesId: series.id });
  }
</script>

<SeriesPicker
  series={gallery.series}
  onSelect={select}
  {eligibility}
  {ownsSeries}
  onCreate={() => nav.push({ name: 'createSeries' })}
  onManage={() => nav.push({ name: 'mySeries' })}
/>
