<script lang="ts">
  import SeriesPicker from '../lib/components/picker/SeriesPicker.svelte';
  import { gallery, rememberSeries } from '../lib/stores/gallery.svelte';
  import { nav } from '../lib/stores/nav.svelte';
  import type { Series } from '../lib/types/api';

  function select(series: Series): void {
    rememberSeries(series.id);
    nav.push({ name: 'home', seriesId: series.id });
  }
</script>

<SeriesPicker series={gallery.series} onSelect={select} />
