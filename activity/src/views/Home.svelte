<script lang="ts">
  import { onMount, untrack } from 'svelte';

  import Calendar from '../lib/components/calendar/Calendar.svelte';
  import CalendarTools from '../lib/components/calendar/CalendarTools.svelte';
  import ArchiveSteps from '../lib/components/home/ArchiveSteps.svelte';
  import Callout from '../lib/components/shared/Callout.svelte';
  import ErrorState from '../lib/components/shared/ErrorState.svelte';
  import Skeleton from '../lib/components/shared/Skeleton.svelte';
  import StatsPanel from '../lib/components/stats/Stats.svelte';
  import Button from '../lib/components/ui/Button.svelte';
  import IconButton from '../lib/components/ui/IconButton.svelte';
  import type { Platform } from '../lib/sdk/types';
  import {
    gallery,
    getApi,
    getGuildId,
    loadDaysIndex,
    refreshAll,
  } from '../lib/stores/gallery.svelte';
  import { focusHeading, nav } from '../lib/stores/nav.svelte';
  import type { DaySummary, Series, Stats } from '../lib/types/api';
  import { accentVar } from '../lib/utils/accent';
  import { buildMonths, weekStartFor, type CalendarMonth } from '../lib/utils/calendar';
  import { channelOf, type SeriesChannel } from '../lib/utils/channel';
  import { describeError, isRetryable } from '../lib/utils/errors';

  interface Props {
    series: Series;
    userId: string;
    canGoBack: boolean;
    /** Which Discord client this is: the archive steps differ. */
    platform?: Platform;
    /** What the application is called in Discord: the steps name it. */
    appName?: string;
    /** The series was just created: say so above the first steps. */
    created?: boolean;
    /**
     * Scrolls to (and focuses) the cell holding this day, once, e.g. the day
     * the viewer was on when it closed. A new object each time.
     */
    reveal?: { day: number } | null;
  }
  let {
    series,
    userId,
    canGoBack,
    platform = 'desktop',
    appName,
    created = false,
    reveal = null,
  }: Props = $props();

  const uid = $props.id();
  const accent = $derived(accentVar(series.id));
  const hasDays = $derived((series.max_day ?? 0) > 0);
  const isOwner = $derived(series.is_owner ?? series.creator_id === userId);
  // The owner's revoked series is listed so it doesn't just vanish, but its
  // days and stats no longer load: explain instead of fetching.
  const revoked = $derived(series.state === 'revoked');
  const canCreate = $derived(gallery.eligibility?.can_create ?? false);
  const showCreate = $derived(canCreate || gallery.eligibilityStatus === 'failed');
  // Captured on arrival: the note belongs to the moment of creation.
  const justCreated = untrack(() => created);
  const weekStart = weekStartFor();

  // --- data -------------------------------------------------------------
  // Each load re-runs when the series changes, after a refresh (`epoch`)
  // and on Try again. A refresh keeps the old data on screen until the new
  // data arrives, and keeps it if the refetch fails.

  let statsLoad = $state<{ id: number; value: Stats } | null>(null);
  let statsError = $state<unknown>(null);
  let statsAttempt = $state(0);
  const stats = $derived(statsLoad?.id === series.id ? statsLoad.value : null);

  $effect(() => {
    const id = series.id;
    void gallery.epoch;
    void statsAttempt;
    if (!hasDays || revoked) return;
    let cancelled = false;
    statsError = null;
    getApi()
      .getStats(getGuildId(), id)
      .then((value) => {
        if (!cancelled) statsLoad = { id, value };
      })
      .catch((e: unknown) => {
        if (!cancelled) statsError = e;
      });
    return () => {
      cancelled = true;
    };
  });

  // The whole archived-day index, cached per series by the store. Also what
  // the viewer's gap-aware navigation reads.
  let indexLoad = $state<{ id: number; value: DaySummary[] } | null>(null);
  let indexError = $state<unknown>(null);
  let indexAttempt = $state(0);
  const index = $derived(indexLoad?.id === series.id ? indexLoad.value : null);

  $effect(() => {
    const id = series.id;
    const maxDay = series.max_day ?? 0;
    void gallery.epoch;
    void indexAttempt;
    if (!hasDays || revoked) return;
    let cancelled = false;
    indexError = null;
    loadDaysIndex(id, maxDay)
      .then((value) => {
        if (!cancelled) indexLoad = { id, value };
      })
      .catch((e: unknown) => {
        if (!cancelled) indexError = e;
      });
    return () => {
      cancelled = true;
    };
  });

  // The owner's archive steps name the series' channel, and only the owner's
  // own list says what it is called, or that it is gone (deleted, or hidden
  // from leaf): then the steps name none and a note above them says so.
  // Asked again after a refresh, since a channel can go while leaf is open.
  // Until it answers, and if it fails, the steps read fine without a name.
  let channel = $state<({ id: number } & SeriesChannel) | null>(null);
  const channelName = $derived(channel?.id === series.id ? channel.name : null);
  const channelGone = $derived(channel?.id === series.id && channel.gone);
  $effect(() => {
    const id = series.id;
    void gallery.epoch;
    if (!isOwner || revoked) return;
    let cancelled = false;
    getApi()
      .listMySeries(getGuildId())
      .then((mine) => {
        const own = mine.find((s) => s.id === id);
        if (own && !cancelled) channel = { id, ...channelOf(own) };
      })
      .catch(() => {
        /* keep the generic wording */
      });
    return () => {
      cancelled = true;
    };
  });

  const layout = $derived(
    index ? buildMonths(index, { timeZone: series.timezone, weekStart }) : null,
  );
  const months = $derived(
    (layout?.items ?? [])
      .filter((item): item is CalendarMonth => item.kind === 'month')
      .map(({ key, label }) => ({ key, label }))
      .reverse(),
  );
  const days = $derived(index?.map((d) => d.day) ?? []);
  const newestPost = $derived(
    series.last_posted_at ??
      index?.reduce<number | null>(
        (max, d) => (max === null || d.posted_at > max ? d.posted_at : max),
        null,
      ) ??
      null,
  );

  // --- actions ----------------------------------------------------------

  let root = $state<HTMLElement>();
  onMount(() => focusHeading(root));

  function openDay(day: number): void {
    // While the viewer is open the calendar is inert; this is a second guard
    // against stacking one viewer on another.
    if (nav.current.name === 'viewer') return;
    nav.push({ name: 'viewer', seriesId: series.id, day });
  }

  function jumpToMonth(key: string): void {
    const find = (): HTMLElement | null =>
      root?.querySelector<HTMLElement>(`[data-month="${CSS.escape(key)}"]`) ?? null;
    const target = find();
    if (!target) return;
    target.scrollIntoView({ block: 'start' });
    target.querySelector<HTMLElement>('h2')?.focus({ preventScroll: true });
    // Months scrolled past were laid out at an estimated height and take
    // their real one once drawn, which can shift the target: aim again.
    requestAnimationFrame(() => {
      requestAnimationFrame(() => find()?.scrollIntoView({ block: 'start' }));
    });
  }

  let note = $state('');
  let noteTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => () => clearTimeout(noteTimer));

  function say(text: string, forMs = 0): void {
    note = text;
    clearTimeout(noteTimer);
    if (forMs > 0) noteTimer = setTimeout(() => (note = ''), forMs);
  }

  async function refresh(): Promise<void> {
    say('');
    const ok = await refreshAll();
    say(ok ? 'Up to date.' : 'Couldn’t refresh. Check your connection and try again.', 4_000);
  }

  async function checkAgain(): Promise<void> {
    say('');
    const ok = await refreshAll();
    if (!ok) say('Couldn’t check. Check your connection and try again.');
    else if (!hasDays) say('Nothing archived yet. Archive a post in chat, then check again.');
  }

  // One-shot: a later index refresh must not scroll back to an old target.
  let revealed: object | null = null;
  $effect(() => {
    const target = reveal;
    if (!target || !index || target === revealed) return;
    revealed = target;
    untrack(() => {
      const cell = root?.querySelector<HTMLElement>(`[data-days~="${target.day}"]`);
      cell?.scrollIntoView({ block: 'nearest' });
      cell?.focus({ preventScroll: true });
    });
  });

  /**
   * The owner's sprout banner. A sprout is hidden from everyone else until
   * the threshold; after that its privacy decides, and "Only me" keeps it
   * the owner's alone, so the threshold changes nothing there.
   */
  const sproutNote = $derived.by((): { title: string; body: string } | null => {
    const { sprout, privacy } = series;
    if (!sprout || !isOwner) return null;
    const progress = `${sprout.threshold} days are archived (${sprout.archived} so far)`;
    if (privacy === 'creator_only') {
      return {
        title: '🌱 Only you can see this series',
        body: `Its privacy is Only me, so that stays the same after ${progress}. To share it, change its privacy in Series settings.`,
      };
    }
    const title = '🌱 Only you can see this series for now';
    if (privacy === 'public') {
      return { title, body: `Everyone in the server can see it once ${progress}.` };
    }
    if (privacy === 'role_gated') {
      return { title, body: `Members with its role can see it once ${progress}.` };
    }
    return {
      title,
      body: `It’s hidden from everyone else until ${progress}. After that, its privacy setting decides who can see it.`,
    };
  });
</script>

<main class="home" style="--accent:{accent}" bind:this={root}>
  <header class="bar">
    {#if canGoBack}
      <IconButton
        ariaLabel="Back to series list"
        variant="solid"
        icon="back"
        onclick={() => nav.back()}
      />
    {/if}
    <span class="emoji" aria-hidden="true">{series.emoji}</span>
    <h1 tabindex="-1" title={series.name}>{series.name}</h1>
    {#if showCreate}
      <!-- The full CTA lives on the picker; a phone-width bar only has room
           for the icon. -->
      <span class="create-wide">
        <Button size="sm" variant="secondary" onclick={() => nav.push({ name: 'createSeries' })}>
          Start a series
        </Button>
      </span>
      <span class="create-narrow">
        <IconButton
          ariaLabel="Start a series"
          variant="solid"
          icon="plus"
          onclick={() => nav.push({ name: 'createSeries' })}
        />
      </span>
    {/if}
    {#if isOwner}
      <IconButton
        ariaLabel="Series settings"
        variant="solid"
        icon="gear"
        onclick={() => nav.push({ name: 'seriesSettings', seriesId: series.id })}
      />
    {/if}
  </header>

  {#if revoked}
    <Callout title="A server admin revoked this series" tone="warning">
      It’s hidden from everyone and can’t take new posts. Its days are kept, so ask a server admin
      to restore it.
    </Callout>
  {:else if sproutNote}
    <Callout title={sproutNote.title}>{sproutNote.body}</Callout>
  {/if}
  {#if channelGone && !revoked}
    <Callout title="This series’ channel is gone" tone="warning">
      It was deleted or hidden from leaf, so nothing posted there can be archived.
      {#snippet action()}
        <Button
          size="sm"
          variant="secondary"
          onclick={() => nav.push({ name: 'seriesSettings', seriesId: series.id })}
        >
          Choose another channel
        </Button>
      {/snippet}
    </Callout>
  {/if}

  {#if revoked}
    <!-- Nothing more to show: days and stats are not served for it. -->
  {:else if !hasDays && isOwner}
    <section class="onboard" aria-labelledby="{uid}-start">
      {#if justCreated}<p class="eyebrow">Series created</p>{/if}
      <h2 id="{uid}-start">Archive your first post</h2>
      <ArchiveSteps {platform} {channelName} {appName} />
      <div class="actions">
        <Button variant="primary" disabled={gallery.refreshing} onclick={() => void checkAgain()}>
          {gallery.refreshing ? 'Checking…' : 'Check again'}
        </Button>
      </div>
      <p class="note" role="status">{note}</p>
    </section>
  {:else if !hasDays}
    <Callout title="No days yet">
      Nothing has been archived in this series yet.
      {#snippet action()}
        <Button
          size="sm"
          variant="secondary"
          disabled={gallery.refreshing}
          onclick={() => void checkAgain()}
        >
          {gallery.refreshing ? 'Checking…' : 'Check again'}
        </Button>
      {/snippet}
    </Callout>
    <p class="note" role="status">{note}</p>
  {:else}
    <div class="content">
      <div class="tools">
        {#if index}
          <CalendarTools
            {days}
            {months}
            refreshing={gallery.refreshing}
            onOpenDay={openDay}
            onJumpMonth={jumpToMonth}
            onRefresh={() => void refresh()}
          />
        {:else if !indexError}
          <Skeleton height="var(--touch-target)" radius="var(--radius-pill)" label="" />
        {/if}
        <p class="note" role="status">{note}</p>
      </div>
      <div class="side">
        {#if !stats && statsError}
          <ErrorState
            title="Couldn’t load the stats"
            message={describeError(statsError)}
            onRetry={isRetryable(statsError) ? () => (statsAttempt += 1) : undefined}
          />
        {:else}
          <StatsPanel {stats} lastPostedAt={newestPost} />
        {/if}
        {#if isOwner}
          <details class="howto">
            <summary>How to archive a post</summary>
            <ArchiveSteps {platform} {channelName} {appName} />
          </details>
        {/if}
      </div>
      <div class="main">
        {#if index}
          {#if index.length > 0}
            <Calendar {index} {layout} {weekStart} timeZone={series.timezone} onOpenDay={openDay} />
          {:else}
            <Callout title="No days yet">Nothing is archived in this series right now.</Callout>
          {/if}
        {:else if indexError}
          <ErrorState
            title="Couldn’t load the calendar"
            message={describeError(indexError)}
            onRetry={isRetryable(indexError) ? () => (indexAttempt += 1) : undefined}
          />
        {:else}
          <Skeleton height="340px" radius="var(--radius-xl)" label="Loading the calendar" />
        {/if}
      </div>
    </div>
  {/if}
</main>

<style>
  .home {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-md);
    width: 100%;
    max-width: 64rem;
    margin: 0 auto;
    padding: var(--space-md);
  }
  /* Sticky under Discord's top inset: at the top of the page the bar sits
   * below it like everything else; once stuck, its own padding covers the
   * inset strip so content never shows through above it. */
  .bar {
    position: sticky;
    top: 0;
    z-index: 1;
    display: flex;
    gap: var(--space-xs);
    align-items: center;
    min-width: 0;
    min-height: calc(var(--appbar-h) + var(--safe-top));
    margin-top: calc(-1 * var(--safe-top));
    padding-top: var(--safe-top);
    background: var(--canvas);
  }
  .emoji {
    flex: none;
    font-size: 1.5rem;
  }
  h1 {
    flex: 1 1 0;
    min-width: 0;
    margin: 0;
    overflow: hidden;
    font-size: var(--fs-card-title);
    font-weight: var(--fw-display);
    letter-spacing: var(--tracking-display);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* Focused by script when the screen appears; a ring there means nothing. */
  h1:focus {
    outline: none;
  }
  .create-wide {
    display: none;
  }
  .create-narrow {
    display: contents;
  }
  @media (min-width: 560px) {
    .create-wide {
      display: contents;
    }
    .create-narrow {
      display: none;
    }
  }
  .onboard {
    display: grid;
    gap: var(--space-sm);
    max-width: 36rem;
    padding: var(--space-lg);
    background: var(--surface-1);
    border: 1px solid var(--hairline);
    border-left: 2px solid var(--accent);
    border-radius: var(--radius-xl);
  }
  .onboard h2 {
    margin: 0;
    font-size: var(--fs-subhead);
    font-weight: var(--fw-display);
    letter-spacing: var(--tracking-display);
  }
  .eyebrow {
    margin: 0;
    color: var(--success);
    font-size: var(--fs-eyebrow);
    font-weight: var(--fw-emphasis);
    letter-spacing: 0.6px;
    text-transform: uppercase;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-xs);
    margin-top: var(--space-xs);
  }
  /* Status lines stay in the page (so a screen reader hears new text), and
   * while empty they take back the gap their container gives them. */
  .note {
    margin: 0;
    color: var(--ink-muted);
    font-size: var(--fs-caption);
  }
  .home > .note:empty {
    margin-top: calc(-1 * var(--space-md));
  }
  .onboard .note:empty {
    margin-top: calc(-1 * var(--space-sm));
  }
  .tools .note:empty {
    margin-top: calc(-1 * var(--space-xs));
  }
  .content {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    grid-template-areas: 'tools' 'side' 'main';
    gap: var(--space-lg);
  }
  .tools {
    display: grid;
    grid-area: tools;
    gap: var(--space-xs);
    min-width: 0;
  }
  .side {
    display: grid;
    grid-area: side;
    gap: var(--space-md);
    align-content: start;
    min-width: 0;
  }
  .main {
    grid-area: main;
    min-width: 0;
  }
  .howto {
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
  }
  .howto summary {
    width: fit-content;
    padding: 0 var(--space-xs);
    margin-left: calc(-1 * var(--space-xs));
    color: var(--link);
    font-weight: var(--fw-emphasis);
    line-height: var(--touch-target);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  /* Phones narrower than 375px: more width for the calendar's seven columns,
   * so a day stays a 44px target at 360px. Below 345px nothing fits seven of
   * those; the tightest spacing gets as close as it can (see MonthGrid). */
  @media (max-width: 374px) {
    .home {
      padding-inline: var(--space-sm);
    }
  }
  @media (max-width: 344px) {
    .home {
      padding-inline: var(--space-xs);
    }
  }
  /* Desktop (DESIGN.md --bp-md 960px): calendar and tools, sticky sidebar. */
  @media (min-width: 960px) {
    .content {
      grid-template-columns: minmax(0, 1fr) 16rem;
      grid-template-areas: 'tools side' 'main side';
      grid-template-rows: auto 1fr;
      align-items: start;
    }
    .side {
      position: sticky;
      top: calc(var(--appbar-h) + var(--safe-top) + var(--space-md));
    }
  }
  @media (hover: hover) {
    .howto summary:hover {
      text-decoration: underline;
    }
  }
</style>
