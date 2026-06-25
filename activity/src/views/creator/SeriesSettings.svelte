<script lang="ts">
  // Owner-only settings for one series. Loads the editable snapshot plus the
  // guild's options (channels/roles), saves via PATCH, and refreshes the
  // gallery so the picker/home reflect renamed emoji etc.
  import SeriesSettingsForm from '../../lib/components/creator/SeriesSettingsForm.svelte';
  import Callout from '../../lib/components/shared/Callout.svelte';
  import Skeleton from '../../lib/components/shared/Skeleton.svelte';
  import IconButton from '../../lib/components/ui/IconButton.svelte';
  import { ApiError } from '../../lib/api/client';
  import { getApi, getGuildId, refreshSeries } from '../../lib/stores/gallery.svelte';
  import { nav } from '../../lib/stores/nav.svelte';
  import type { SeriesOptions, SeriesSettings, UpdateSeriesInput } from '../../lib/types/api';
  import { seriesErrorMessage } from '../../lib/utils/labels';

  interface Props {
    seriesId: number;
  }
  let { seriesId }: Props = $props();

  let settings = $state<SeriesSettings | null>(null);
  let options = $state<SeriesOptions | null>(null);
  let loadFailed = $state(false);
  let saving = $state(false);
  let saved = $state(false);
  let saveError = $state<string | null>(null);

  $effect(() => {
    const api = getApi();
    const gid = getGuildId();
    const id = seriesId;
    let cancelled = false;
    Promise.all([api.getSettings(gid, id), api.getOptions(gid)])
      .then(([s, o]) => {
        if (cancelled) return;
        settings = s;
        options = o;
      })
      .catch(() => {
        if (!cancelled) loadFailed = true;
      });
    return () => {
      cancelled = true;
    };
  });

  async function save(patch: UpdateSeriesInput): Promise<void> {
    saving = true;
    saved = false;
    saveError = null;
    try {
      settings = await getApi().patchSeries(getGuildId(), seriesId, patch);
      await refreshSeries();
      saved = true;
    } catch (e) {
      saveError =
        e instanceof ApiError
          ? seriesErrorMessage(e.code, 'Couldn’t save. Try again.')
          : 'Couldn’t save. Try again.';
    } finally {
      saving = false;
    }
  }
</script>

<div class="view">
  <header class="bar">
    <IconButton ariaLabel="Back" variant="solid" onclick={() => nav.back()}>←</IconButton>
    <h1>{settings ? settings.name : 'Series settings'}</h1>
  </header>

  {#if loadFailed}
    <Callout title="Couldn’t load settings">
      You may not own this series, or it’s unavailable here.
    </Callout>
  {:else if !settings || !options}
    <Skeleton height="360px" radius="var(--radius-xl)" />
  {:else}
    <SeriesSettingsForm {settings} {options} {saving} {saved} error={saveError} onSave={save} />
  {/if}
</div>

<style>
  .view {
    display: grid;
    gap: var(--space-md);
    width: 100%;
    max-width: 44rem;
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
