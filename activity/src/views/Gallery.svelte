<script lang="ts">
  import { onMount, tick } from 'svelte';

  import ErrorState from '../lib/components/shared/ErrorState.svelte';
  import Minimisable from '../lib/components/shared/Minimisable.svelte';
  import Skeleton from '../lib/components/shared/Skeleton.svelte';
  import IconButton from '../lib/components/ui/IconButton.svelte';
  import { closeActivity, onForeground } from '../lib/sdk/actions';
  import { parseCustomId } from '../lib/sdk/customId';
  import type { Session } from '../lib/sdk/handshake';
  import {
    gallery,
    initGallery,
    lastSeries,
    peekThumb,
    refreshAll,
    rememberSeries,
    takeLaunchIntent,
  } from '../lib/stores/gallery.svelte';
  import { layout, setLayoutMode } from '../lib/stores/layout.svelte';
  import {
    focusHeading,
    nav,
    resolveTarget,
    stackFor,
    startTarget,
    type StartTarget,
    type View,
  } from '../lib/stores/nav.svelte';
  import type { LaunchIntent } from '../lib/types/api';
  import Home from './Home.svelte';
  import Picker from './Picker.svelte';

  interface Props {
    session: Session;
  }
  let { session }: Props = $props();

  const userId = $derived(session.user.id);
  const view = $derived(nav.current);
  const activeSeries = $derived(
    view.name === 'home' || view.name === 'viewer'
      ? (gallery.series.find((s) => s.id === view.seriesId) ?? null)
      : null,
  );

  /** Discord does not confirm a close; this long without one, say how. */
  const CLOSE_HINT_AFTER_MS = 1_500;
  /** Automatic refreshes on return skip data younger than this. */
  const FOREGROUND_REFRESH_AFTER_MS = 30_000;
  /** How often a minimised leaf asks whether "Open gallery" was pressed in chat. */
  const TILE_INTENT_EVERY_MS = 4_000;

  // --- lazy chunks ------------------------------------------------------
  // The day viewer and the creator screens are separate chunks so the first
  // screen arrives sooner (PERF.md budget). The viewer is fetched as soon as
  // the gallery mounts, so it is there by the first tap; the creator screens
  // on first use. An import can fail on a weak connection, or for good once
  // leaf was updated under an open Activity (the old file is gone and a
  // reload is impossible inside Discord): one Try again, then say so.

  type ChunkState = 'idle' | 'loading' | 'failed';

  function lazyChunk<T>(what: string, load: () => Promise<T>) {
    const chunk = $state<{ module: T | null; state: ChunkState; failures: number }>({
      module: null,
      state: 'idle',
      failures: 0,
    });
    function fetchChunk(quiet: boolean): void {
      if (chunk.module || chunk.state === 'loading') return;
      chunk.state = 'loading';
      load()
        .then((m) => {
          chunk.module = m;
          chunk.state = 'idle';
        })
        .catch((e: unknown) => {
          console.error(`leaf: loading ${what} failed`, e);
          if (quiet) {
            chunk.state = 'idle';
            return;
          }
          chunk.failures += 1;
          chunk.state = 'failed';
        });
    }
    return {
      chunk,
      /** Loads the chunk for a screen that needs it now. */
      load: (): void => fetchChunk(false),
      /** Loads it ahead of need; a failure leaves it to the real load. */
      prefetch: (): void => fetchChunk(true),
    };
  }

  const creator = lazyChunk('the creator screens', () => import('./creator/entry'));
  const viewer = lazyChunk('the day viewer', () => import('./Viewer.svelte'));
  const Viewer = $derived(viewer.chunk.module?.default ?? null);

  function isCreatorView(name: View['name']): boolean {
    return name === 'createSeries' || name === 'mySeries' || name === 'seriesSettings';
  }

  $effect(() => {
    if (isCreatorView(view.name) && creator.chunk.state === 'idle') creator.load();
  });
  $effect(() => {
    if (view.name === 'viewer' && viewer.chunk.state === 'idle') viewer.load();
  });

  /** Puts focus in a stand-in overlay, since everything under it is inert. */
  function focusOnMount(node: HTMLElement): void {
    node.focus();
  }

  // --- closing leaf -----------------------------------------------------

  let closing = $state(false);
  let closeHint = $state(false);
  let closeTimer: ReturnType<typeof setTimeout> | undefined;
  async function close(): Promise<void> {
    closing = await closeActivity();
    clearTimeout(closeTimer);
    closeTimer = setTimeout(() => (closeHint = true), closing ? CLOSE_HINT_AFTER_MS : 0);
  }

  // --- where to open ----------------------------------------------------

  function open(target: StartTarget | null): void {
    if (target) rememberSeries(target.seriesId);
    nav.reset(...stackFor(target));
  }

  /**
   * Until the first view is chosen the loading placeholder stays up, so
   * nothing the person starts in a list that is about to be replaced is
   * thrown away when the launch intent answers.
   */
  let opening = $state(true);

  /** Loads the gallery and opens it where the launch asked. Also the Retry. */
  async function start(): Promise<void> {
    opening = true;
    try {
      await initGallery(session);
      if (gallery.status !== 'ready') return;
      const since = nav.version;
      // The intent was requested with the series list; this waits at most a
      // few seconds for it.
      const intent = await takeLaunchIntent();
      if (nav.version !== since) return;
      open(
        startTarget(gallery.series, {
          intent,
          link: parseCustomId(session.customId),
          channelId: session.channelId,
          remembered: lastSeries(),
        }),
      );
    } finally {
      opening = false;
    }
  }

  /**
   * An "Open gallery" press in chat while leaf was in the background. It
   * can answer seconds after the return, so it is dropped if the person has
   * moved on since. The list on screen may be older than the press: a
   * series it lacks, or a day past the newest one it knows (the post that
   * was just archived), gets one refresh before the intent is resolved.
   * Resolves to whether it opened anything.
   */
  async function follow(intent: LaunchIntent | null, since: number): Promise<boolean> {
    if (!intent || nav.version !== since) return false;
    let target = resolveTarget(gallery.series, intent);
    if (!target || (intent.day !== null && target.day === null)) {
      await refreshAll();
      if (nav.version !== since) return false;
      target = resolveTarget(gallery.series, intent);
    }
    if (target) open(target);
    return target !== null;
  }

  onMount(() => {
    viewer.prefetch();
    void start();
    return () => clearTimeout(closeTimer);
  });

  // Back in front (out of picture-in-picture, back from chat): bring the
  // data up to date and follow a press in chat. Neither blocks the screen.
  // The same subscription says when Discord puts leaf in picture-in-picture.
  $effect(() =>
    onForeground(() => {
      // While start() is still choosing the first view, it has the intent.
      if (gallery.status !== 'ready' || opening) return;
      const since = nav.version;
      void refreshAll({ ifOlderThanMs: FOREGROUND_REFRESH_AFTER_MS });
      void takeLaunchIntent().then((intent) => follow(intent, since));
    }, setLayoutMode),
  );

  // --- a press in chat while minimised ----------------------------------
  // Discord for Android does not bring a running Activity forward: with
  // leaf shrunk to a tile, "Open gallery" in chat does nothing anyone can
  // see, and leaf is told nothing until the tile is tapped. So while it is
  // a tile, and only then, leaf asks every few seconds. What was pressed
  // opens behind the card, as on any return, and the card says to tap.
  // Nothing else in leaf polls (idle is zero): this stops with the tile, or
  // with the session.

  /** The stack entry a press in chat opened behind the card, until leaf is opened. */
  let waiting = $state<number | null>(null);

  $effect(() => {
    if (!layout.tile || gallery.status !== 'ready' || opening) return;
    let live = true;
    let asking = false;
    const timer = setInterval(() => {
      // An answer still on its way (a slow connection) is not asked for twice.
      if (asking) return;
      asking = true;
      const since = nav.version;
      void takeLaunchIntent()
        .then((intent) => follow(intent, since))
        .then((opened) => {
          if (opened && live) waiting = nav.key;
        })
        // A press that could not be followed is lost; the next is still asked for.
        .catch((e: unknown) => console.error('leaf: following a press in chat failed', e))
        .finally(() => (asking = false));
    }, TILE_INTENT_EVERY_MS);
    return () => {
      live = false;
      clearInterval(timer);
      waiting = null;
    };
  });

  // --- view changes -----------------------------------------------------
  // A new screen starts at the top; going back puts the old one where it
  // was left. The day viewer is an overlay: opening and closing it leaves
  // the page alone. Each screen focuses its own heading on mount.

  let shown: { key: number; name: View['name'] } | null = null;
  $effect(() => {
    const now = { key: nav.key, name: view.name };
    const before = shown;
    shown = now;
    if (!before || before.key === now.key) return;
    if (before.name === 'viewer' || now.name === 'viewer') return;
    const y = nav.savedScroll;
    if (y === 0) {
      window.scrollTo(0, 0);
      return;
    }
    // Content that loads from a cache lands a moment after the screen.
    requestAnimationFrame(() => window.scrollTo(0, y));
  });

  // --- the day viewer over Home -------------------------------------------

  let homeLayer = $state<HTMLElement>();
  let reveal = $state<{ day: number } | null>(null);

  /**
   * Closes the viewer. `lastDay` is the day it was showing, when it says:
   * Home then scrolls that day's cell into view. Typed loosely because a
   * close button wired straight to this passes its click event instead.
   */
  function closeViewer(lastDay?: unknown): void {
    // Lift `inert` before the viewer hands focus back into Home.
    if (homeLayer) homeLayer.inert = false;
    reveal = typeof lastDay === 'number' && Number.isInteger(lastDay) ? { day: lastDay } : null;
    nav.back();
    void tick().then(() => {
      if (document.activeElement === document.body) focusHeading(homeLayer);
    });
  }

  /**
   * Escape leaves the stand-in too; the real viewer handles its own. Not
   * while leaf is a tile: what is put away stays as it was left.
   */
  function onWindowKeydown(e: KeyboardEvent): void {
    if (layout.tile) return;
    if (e.key === 'Escape' && view.name === 'viewer' && !Viewer && activeSeries) closeViewer();
  }

  function backToList(): void {
    nav.reset({ name: 'picker' });
  }

  // --- minimised --------------------------------------------------------
  // What the card says while Discord has leaf shrunk to a tile
  // (Minimisable.svelte): the day or the series on screen, with a picture
  // when one is at hand, and otherwise just leaf.

  /** The day the open viewer has paged to; it starts on the one it was opened on. */
  let paged = $state<{ key: number; day: number } | null>(null);

  interface Tile {
    mark?: string;
    title?: string;
    detail?: string;
    image?: string | null;
    cue?: string;
  }
  const tile = $derived.by((): Tile => {
    if (!layout.tile) return {};
    if (gallery.status === 'expired') return { detail: 'Session ended' };
    if (gallery.status === 'loading' || opening) return { detail: 'Opening…' };
    if (gallery.status === 'error') return { detail: 'Didn’t load' };
    // The list, a creator screen, or a series that is gone.
    if (!activeSeries) return {};
    const open = view.name === 'viewer' ? (paged?.key === nav.key ? paged.day : view.day) : null;
    const day = open ?? activeSeries.max_day;
    // Looked up again when a day index arrives in the cache.
    void gallery.indexed;
    return {
      mark: activeSeries.emoji,
      title: activeSeries.name,
      detail: day === null ? 'No days yet' : `Day ${day}`,
      image: peekThumb(activeSeries.id, open),
      // A press in chat put this here: only a tap on the tile shows it.
      ...(waiting === nav.key ? { cue: 'Tap to open' } : {}),
    };
  });

  // "Check again" on an unavailable series: say what it found, since a
  // series that is still missing leaves the screen exactly as it was.
  let unavailable = $state<{ key: number; text: string } | null>(null);
  const unavailableNote = $derived(unavailable?.key === nav.key ? unavailable.text : '');

  async function checkSeriesAgain(): Promise<void> {
    const key = nav.key;
    unavailable = null;
    const ok = await refreshAll();
    if (!ok) {
      unavailable = { key, text: 'Couldn’t check. Check your connection and try again.' };
    } else if (!activeSeries) {
      unavailable = { key, text: 'Still not available. It may have been removed or hidden.' };
    }
  }
</script>

<svelte:window onkeydown={onWindowKeydown} />

<!-- Every screen below is one <main> (the views bring their own), so the
     page always has a main landmark and nothing sits outside it. The day
     viewer is a dialog beside it. A state that fills the screen passes
     `page` to ErrorState, which makes its title the page's heading. -->

{#snippet closeNote()}
  {#if closeHint}
    <p class="hint">If leaf is still open, close it with Discord’s own controls.</p>
  {/if}
{/snippet}

<!-- A chunk that did not arrive: one Try again, then the only real fix. The
     way out is the Back or Close control above it, so it isn't repeated. -->
{#snippet chunkFailed(failures: number, retry: () => void)}
  {#if failures < 2}
    <ErrorState
      page
      title="Couldn’t open this screen"
      message="Check your connection and try again."
      onRetry={retry}
    />
  {:else}
    <ErrorState
      page
      title="This screen didn’t load"
      message="leaf may have been updated, or the connection dropped. Close leaf and open it again."
      retryLabel="Close leaf"
      onRetry={closing ? undefined : () => void close()}
    />
    {@render closeNote()}
  {/if}
{/snippet}

<Minimisable {...tile}>
  {#if gallery.status === 'expired'}
    <main class="state">
      <ErrorState
        page
        title="Your session has ended"
        message="Close leaf and open it again to keep browsing."
        retryLabel="Close leaf"
        onRetry={closing ? undefined : () => void close()}
      />
      {@render closeNote()}
    </main>
  {:else if gallery.status === 'loading' || (gallery.status === 'ready' && opening)}
    <!-- Shaped like the series list, the usual first screen. -->
    <main class="placeholder">
      <Skeleton width="40%" height="28px" label="Loading the gallery" />
      <Skeleton height="76px" radius="var(--radius-xl)" label="" />
      <Skeleton height="76px" radius="var(--radius-xl)" label="" />
      <Skeleton height="76px" radius="var(--radius-xl)" label="" />
    </main>
  {:else if gallery.status === 'error'}
    <main class="state">
      <ErrorState
        page
        title="Couldn’t load the gallery"
        message={gallery.error}
        onRetry={gallery.errorKind === 'no_guild' ? undefined : () => void start()}
      />
    </main>
  {:else if view.name === 'picker'}
    <Picker {userId} channelId={session.channelId} />
  {:else if isCreatorView(view.name)}
    {@const views = creator.chunk.module}
    {#if views}
      {#if view.name === 'createSeries'}
        <views.CreateSeries />
      {:else if view.name === 'mySeries'}
        <views.MySeries />
      {:else if view.name === 'seriesSettings'}
        <views.SeriesSettings seriesId={view.seriesId} />
      {/if}
    {:else}
      <main class="shell">
        <header class="bar">
          <IconButton ariaLabel="Back" variant="solid" icon="back" onclick={() => nav.back()} />
        </header>
        {#if creator.chunk.state === 'failed'}
          {@render chunkFailed(creator.chunk.failures, creator.load)}
        {:else}
          <Skeleton height="48px" radius="var(--radius-lg)" label="Loading" />
          <Skeleton height="160px" radius="var(--radius-xl)" label="" />
        {/if}
      </main>
    {/if}
  {:else if activeSeries}
    <!-- While the viewer is open, everything under it is inert: no Tab stop,
       no screen reader browsing, no second viewer from a covered cell. -->
    <div bind:this={homeLayer} inert={view.name === 'viewer'}>
      <Home
        series={activeSeries}
        {userId}
        canGoBack={nav.canGoBack}
        platform={session.platform}
        appName={session.appName}
        created={view.name === 'home' && view.created === true}
        {reveal}
      />
    </div>
    {#if view.name === 'viewer'}
      <!-- Keyed by the stack entry: a deep link to another day while the
         viewer is open replaces it rather than leaving it on the old day. -->
      {#key nav.key}
        {#if Viewer}
          <Viewer
            series={activeSeries}
            day={view.day}
            platform={session.platform}
            onClose={closeViewer}
            onDay={(day) => (paged = { key: nav.key, day })}
          />
        {:else}
          <div
            class="overlay"
            role="dialog"
            aria-modal="true"
            aria-label={`Day ${view.day}, ${activeSeries.name}`}
            tabindex="-1"
            use:focusOnMount
          >
            <!-- An import has no timeout: Close (and Escape) work while it
               loads, not only once it has failed. -->
            <div class="overlay-bar">
              <IconButton
                ariaLabel="Close"
                variant="solid"
                icon="close"
                onclick={() => closeViewer()}
              />
            </div>
            {#if viewer.chunk.state === 'failed'}
              {@render chunkFailed(viewer.chunk.failures, viewer.load)}
            {:else}
              <Skeleton width="200px" height="16px" label="Loading the day" />
            {/if}
          </div>
        {/if}
      {/key}
    {/if}
  {:else}
    <main class="state">
      <ErrorState
        page
        title="This series isn’t available"
        message="It may have been removed or hidden, or you may no longer have access to it."
        retryLabel="Check again"
        onRetry={gallery.refreshing ? undefined : () => void checkSeriesAgain()}
        backLabel="Back to series"
        onBack={backToList}
      />
      <p class="hint" role="status">{unavailableNote}</p>
    </main>
  {/if}
</Minimisable>

<style>
  .state {
    display: grid;
    gap: var(--space-sm);
    place-content: center;
    justify-items: center;
    min-height: 60vh;
    padding: var(--space-lg);
  }
  .hint {
    max-width: 32rem;
    margin: 0;
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
  }
  /* A status line stays in the page so new text is announced; while empty
   * it gives back the gap above it. */
  .state > .hint:empty {
    margin-top: calc(-1 * var(--space-sm));
  }
  .placeholder,
  .shell {
    display: grid;
    gap: var(--space-sm);
    width: 100%;
    max-width: 40rem;
    margin: 0 auto;
    padding: var(--space-lg);
  }
  .shell {
    padding: var(--space-md);
  }
  .bar {
    display: flex;
    align-items: center;
    min-height: var(--appbar-h);
  }
  /* Stands in for the viewer's own overlay until its chunk arrives. */
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 10;
    display: grid;
    gap: var(--space-sm);
    place-content: center;
    justify-items: center;
    padding: max(var(--space-lg), var(--safe-top)) max(var(--space-lg), var(--safe-right))
      max(var(--space-lg), var(--safe-bottom)) max(var(--space-lg), var(--safe-left));
    background: var(--canvas);
    overscroll-behavior: contain;
  }
  .overlay:focus {
    outline: none;
  }
  /* Where the viewer's own Close sits once it arrives: top right. */
  .overlay-bar {
    position: absolute;
    inset: 0 0 auto;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    min-height: calc(var(--appbar-h) + var(--safe-top));
    padding: var(--safe-top) max(var(--space-md), var(--safe-right)) 0
      max(var(--space-md), var(--safe-left));
  }
</style>
