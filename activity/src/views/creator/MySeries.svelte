<script lang="ts">
  // The creator's own series, each opening its settings. Reached from the
  // picker's "Manage my series" link. Revoked series are listed too, with
  // what that means, so a series an admin hid does not just vanish.
  import { onMount } from 'svelte';

  import MySeriesCard from '../../lib/components/creator/MySeriesCard.svelte';
  import Callout from '../../lib/components/shared/Callout.svelte';
  import ErrorState from '../../lib/components/shared/ErrorState.svelte';
  import Skeleton from '../../lib/components/shared/Skeleton.svelte';
  import Button from '../../lib/components/ui/Button.svelte';
  import IconButton from '../../lib/components/ui/IconButton.svelte';
  import { gallery, getApi, getGuildId } from '../../lib/stores/gallery.svelte';
  import { focusHeading, nav } from '../../lib/stores/nav.svelte';
  import type { MySeries } from '../../lib/types/api';
  import { describeError, isRetryable } from '../../lib/utils/errors';

  let series = $state<MySeries[] | null>(null);
  let failed = $state<unknown>(null);
  let attempt = $state(0);

  /**
   * The list does not have to say when a series' reminders are failing; its
   * settings do. So each series whose reminders are on is asked about, one
   * at a time, once the list is on screen. Best effort: a card that could
   * not be checked keeps saying "reminders on".
   */
  async function checkReminders(rows: MySeries[], stale: () => boolean): Promise<void> {
    const api = getApi();
    const gid = getGuildId();
    for (const row of rows) {
      // Nothing is sent for a revoked or freeform series, so nothing can fail.
      const sends = row.reminder_enabled && row.state !== 'revoked' && row.cadence !== 'freeform';
      if (!sends || row.reminder_error !== undefined) continue;
      const reason = await api.getSettings(gid, row.id).then(
        (settings) => (settings.reminder_enabled ? settings.reminder_error : undefined),
        () => undefined,
      );
      if (stale()) return;
      if (reason !== undefined) {
        series = (series ?? []).map((s) =>
          s.id === row.id ? { ...s, reminder_error: reason } : s,
        );
      }
    }
  }

  $effect(() => {
    void attempt;
    const api = getApi();
    const gid = getGuildId();
    let cancelled = false;
    failed = null;
    api
      .listMySeries(gid)
      .then((rows) => {
        if (cancelled) return;
        series = rows;
        void checkReminders(rows, () => cancelled);
      })
      .catch((e: unknown) => {
        console.error('leaf: loading your series failed', e);
        if (!cancelled) failed = e;
      });
    return () => {
      cancelled = true;
    };
  });

  // The gallery's list knows each series' privacy and sprout threshold;
  // this screen's own list does not.
  const known = $derived(new Map(gallery.series.map((s) => [s.id, s])));
  const canCreate = $derived(gallery.eligibility?.can_create ?? false);

  function open(s: MySeries): void {
    nav.push({ name: 'seriesSettings', seriesId: s.id });
  }

  let root = $state<HTMLElement>();
  onMount(() => focusHeading(root));
</script>

<main class="view" bind:this={root}>
  <header class="bar">
    <IconButton ariaLabel="Back" variant="solid" icon="back" onclick={() => nav.back()} />
    <h1 tabindex="-1">My series</h1>
  </header>

  {#if series}
    {#if series.length === 0 && canCreate}
      <Callout title="You haven’t started a series here">
        Start one, then add its days from chat.
        {#snippet action()}
          <Button variant="primary" onclick={() => nav.push({ name: 'createSeries' })}>
            Start a series
          </Button>
        {/snippet}
      </Callout>
    {:else if series.length === 0}
      <Callout title="You haven’t started a series here">
        Series you start in this server show up here.
      </Callout>
    {:else}
      <ul class="list">
        {#each series as s (s.id)}
          <li>
            <MySeriesCard
              series={s}
              onOpen={open}
              sprout={known.get(s.id)?.sprout}
              privacy={known.get(s.id)?.privacy}
            />
          </li>
        {/each}
      </ul>
    {/if}
  {:else if failed}
    <ErrorState
      title="Couldn’t load your series"
      message={describeError(failed)}
      onRetry={isRetryable(failed) ? () => (attempt += 1) : undefined}
    />
  {:else}
    <Skeleton height="96px" radius="var(--radius-xl)" label="Loading your series" />
    <Skeleton height="96px" radius="var(--radius-xl)" label="" />
  {/if}
</main>

<style>
  .view {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
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
  /* Focused by script when the screen appears; a ring there means nothing. */
  h1:focus {
    outline: none;
  }
  .list {
    display: grid;
    gap: var(--space-sm);
    margin: 0;
    padding: 0;
    list-style: none;
  }
</style>
