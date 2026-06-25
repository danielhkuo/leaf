<script lang="ts">
  // The "start a series" view: load options + eligibility once, then either
  // block with the server's reasons or run the wizard. A successful create
  // refreshes the gallery and lands on the new series' home.
  import CreateWizard from '../../lib/components/creator/CreateWizard.svelte';
  import ViolationCallout from '../../lib/components/creator/ViolationCallout.svelte';
  import Callout from '../../lib/components/shared/Callout.svelte';
  import Skeleton from '../../lib/components/shared/Skeleton.svelte';
  import IconButton from '../../lib/components/ui/IconButton.svelte';
  import { ApiError } from '../../lib/api/client';
  import { getApi, getGuildId, refreshSeries } from '../../lib/stores/gallery.svelte';
  import { nav } from '../../lib/stores/nav.svelte';
  import type { CreateSeriesInput, Eligibility, SeriesOptions } from '../../lib/types/api';
  import { seriesErrorMessage } from '../../lib/utils/labels';

  let eligibility = $state<Eligibility | null>(null);
  let options = $state<SeriesOptions | null>(null);
  let loadFailed = $state(false);
  let submitting = $state(false);
  let submitError = $state<string | null>(null);

  $effect(() => {
    const api = getApi();
    const gid = getGuildId();
    let cancelled = false;
    Promise.all([api.getEligibility(gid), api.getOptions(gid)])
      .then(([e, o]) => {
        if (cancelled) return;
        eligibility = e;
        options = o;
      })
      .catch(() => {
        if (!cancelled) loadFailed = true;
      });
    return () => {
      cancelled = true;
    };
  });

  async function submit(input: CreateSeriesInput): Promise<void> {
    submitting = true;
    submitError = null;
    try {
      const created = await getApi().createSeries(getGuildId(), input);
      await refreshSeries();
      nav.reset({ name: 'picker' });
      nav.push({ name: 'home', seriesId: created.id });
    } catch (e) {
      submitError =
        e instanceof ApiError
          ? seriesErrorMessage(e.code, 'Couldn’t create the series. Try again.')
          : 'Couldn’t create the series. Try again.';
    } finally {
      submitting = false;
    }
  }
</script>

<div class="view">
  <header class="bar">
    <IconButton ariaLabel="Back" variant="solid" onclick={() => nav.back()}>←</IconButton>
    <h1>Start a series</h1>
  </header>

  {#if loadFailed}
    <Callout title="Couldn’t load this">Try reopening the gallery.</Callout>
  {:else if !eligibility || !options}
    <Skeleton height="320px" radius="var(--radius-xl)" />
  {:else if !eligibility.can_create}
    <ViolationCallout violations={eligibility.violations} />
  {:else}
    <CreateWizard {options} {submitting} error={submitError} onSubmit={submit} />
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
</style>
