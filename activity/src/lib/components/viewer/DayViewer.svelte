<script lang="ts">
  // The day viewer's shell. It stays mounted while days come and go: the top
  // bar (with Close), the arrows and the actions are there while a day loads
  // and when it fails, so stepping through days never drops to a bare
  // skeleton. `day` is null until the day's data arrives; `summary` (the
  // calendar's row for the day) fills the date and a blurred preview until
  // then.
  //
  // Paging goes through a day's files first and crosses to the neighbouring
  // day at the ends, for swipes, the arrows and the arrow keys alike.
  import { openExternalLink, type LinkOutcome } from '../../sdk/actions';
  import type { Day, DaySummary } from '../../types/api';
  import { formatCaption } from '../../utils/caption';
  import { formatPostedAt, isoInstant } from '../../utils/datetime';
  import {
    createGesture,
    pointerCancel,
    pointerDown,
    pointerMove,
    pointerUp,
    type View,
  } from '../../utils/gesture';
  import { clampIndex } from '../../utils/navigation';
  import Button from '../ui/Button.svelte';
  import IconButton from '../ui/IconButton.svelte';
  import Spinner from '../ui/Spinner.svelte';
  import ZoomableImage from './ZoomableImage.svelte';

  interface Props {
    dayNumber: number;
    /** The day's data, or `null` while it loads (or after it failed). */
    day: Day | null;
    /** A sentence to show when the day could not be loaded. */
    error?: string | null | undefined;
    /** The calendar's row for the day: its date and preview before `day` arrives. */
    summary?: DaySummary | null | undefined;
    seriesName: string;
    /** The server's timezone, so the date matches the calendar. */
    timeZone?: string | undefined;
    hasPrev: boolean;
    hasNext: boolean;
    onPrev: () => void;
    onNext: () => void;
    /** Opens another day at random; leave out when there is no other day. */
    onRandom?: (() => void) | undefined;
    onClose: () => void;
    /** Loads the day again: after a failed load, or to renew a photo's address. */
    onRetry?: (() => void) | undefined;
    openLink?: (url: string) => Promise<LinkOutcome>;
  }
  let {
    dayNumber,
    day,
    error = null,
    summary = null,
    seriesName,
    timeZone,
    hasPrev,
    hasNext,
    onPrev,
    onNext,
    onRandom,
    onClose,
    onRetry,
    openLink = openExternalLink,
  }: Props = $props();

  /** Past any real count: "the last file", for a day entered backwards. */
  const LAST = Number.MAX_SAFE_INTEGER;
  const STILL: View = { scale: 1, tx: 0, ty: 0 };
  const LIMITS = { minScale: 1, maxScale: 1 };

  // --- paging -------------------------------------------------------------

  let wanted = $state(0);
  const media = $derived(day?.media ?? []);
  const shown = $derived(clampIndex(wanted, media.length));
  const current = $derived(media[shown]);
  const isVideo = $derived(current?.content_type.startsWith('video/') ?? false);
  const allPhotos = $derived(media.every((m) => m.content_type.startsWith('image/')));
  const fileWord = $derived(allPhotos ? 'photo' : 'file');
  const FileWord = $derived(allPhotos ? 'Photo' : 'File');
  const dots = $derived(media.map((_, i) => i));

  const canBack = $derived(shown > 0 || hasPrev);
  const canForward = $derived(shown < media.length - 1 || hasNext);
  const backLabel = $derived(shown > 0 ? `Previous ${fileWord}` : 'Previous day');
  const forwardLabel = $derived(shown < media.length - 1 ? `Next ${fileWord}` : 'Next day');

  function back(): void {
    if (shown > 0) {
      wanted = shown - 1;
    } else if (hasPrev) {
      // Going backwards lands on the end of the day before.
      wanted = LAST;
      onPrev();
    }
  }
  function forward(): void {
    if (shown < media.length - 1) {
      wanted = shown + 1;
    } else if (hasNext) {
      wanted = 0;
      onNext();
    }
  }
  function random(): void {
    wanted = 0;
    onRandom?.();
  }

  // --- header and caption ---------------------------------------------------

  const postedAt = $derived(day?.posted_at ?? summary?.posted_at);
  const dateText = $derived(
    postedAt === undefined ? '' : formatPostedAt(postedAt, undefined, timeZone),
  );
  const caption = $derived(day ? formatCaption(day.caption, { timeZone }) : '');
  const alt = $derived(caption || `Day ${dayNumber} of ${seriesName}`);

  // Both are remembered per day, so a new day starts collapsed and clean.
  let expandedDay = $state<number | null>(null);
  const expanded = $derived(expandedDay === dayNumber);
  let captionEl = $state<HTMLElement>();
  let clipped = $state(false);

  function measureCaption(): void {
    const el = captionEl;
    if (el && !expanded) clipped = el.scrollHeight > el.clientHeight + 1;
  }
  $effect(() => {
    void caption;
    void expanded;
    measureCaption();
  });
  $effect(() => {
    const el = captionEl;
    if (!el) return;
    const ro = new ResizeObserver(measureCaption);
    ro.observe(el);
    return () => ro.disconnect();
  });

  // --- the original post ----------------------------------------------------

  let linkFailedDay = $state<number | null>(null);
  const linkFailed = $derived(linkFailedDay === dayNumber);
  let copyState = $state<'idle' | 'copied' | 'failed'>('idle');
  const canCopy = typeof navigator !== 'undefined' && navigator.clipboard !== undefined;

  async function openPost(): Promise<void> {
    if (!day) return;
    const asked = dayNumber;
    const outcome = await openLink(day.jump_url);
    // Staying on Discord's "leaving" prompt is the person's choice: say nothing.
    if (outcome === 'failed') {
      copyState = 'idle';
      linkFailedDay = asked;
    } else if (linkFailedDay === asked) {
      linkFailedDay = null;
    }
  }

  async function copyLink(): Promise<void> {
    if (!day) return;
    try {
      await navigator.clipboard.writeText(day.jump_url);
      copyState = 'copied';
    } catch {
      // Discord's iframe may not be allowed the clipboard; the link is
      // selectable for that case.
      copyState = 'failed';
    }
  }

  // --- video ----------------------------------------------------------------

  let videoFailedUrl = $state<string | null>(null);
  const videoFailed = $derived(current !== undefined && videoFailedUrl === current.url);
  /** Bumped by Try again, so the same address gets a new <video>. */
  let videoAttempt = $state(0);

  // A video fails for a dropped connection or an expired address as well as
  // for a format the device can't play, so it gets another go like a photo.
  function retryVideo(): void {
    onRetry?.();
    videoFailedUrl = null;
    videoAttempt += 1;
    // The button that had focus is gone with the note.
    dialogEl?.focus();
  }

  // --- focus and keys -------------------------------------------------------

  // Take focus into the dialog, hand it back on close.
  let dialogEl: HTMLElement | undefined;
  $effect(() => {
    const previous = document.activeElement;
    dialogEl?.focus();
    return () => {
      if (previous instanceof HTMLElement) previous.focus();
    };
  });

  // An arrow is removed when its side runs out. If it had focus, the browser
  // drops focus to the page behind the dialog: bring it back.
  $effect(() => {
    void canBack;
    void canForward;
    const active = document.activeElement;
    if (active === null || active === document.body) dialogEl?.focus();
  });

  function onKeydown(e: KeyboardEvent): void {
    if (e.defaultPrevented) return;
    if (e.key === 'Escape') {
      onClose();
      return;
    }
    if (e.altKey || e.ctrlKey || e.metaKey) return;
    // Arrows belong to a video's seek bar or a field's caret while it has focus.
    const target = e.target;
    if (
      target instanceof Element &&
      target.closest('video, input, textarea, select, [contenteditable]')
    ) {
      return;
    }
    if (e.key === 'ArrowRight') forward();
    else if (e.key === 'ArrowLeft') back();
  }

  // --- swipe on a stage with no photo ---------------------------------------
  // A photo handles its own touches (ZoomableImage), also once it has failed.
  // A video, a note or a loading day still turns the page on a sideways
  // swipe; a tap is left to whatever is under it (a video's play button).

  let gesture = createGesture();
  const center = (e: PointerEvent) => ({ x: e.clientX, y: e.clientY });

  /** The share of a video's height, from its bottom edge, left to its controls. */
  const VIDEO_CONTROLS_SHARE = 0.25;
  const VIDEO_CONTROLS_MIN_PX = 56;

  // A video that fills the stage (a portrait clip on a phone) leaves nothing
  // around it to swipe on, so a touch on the picture counts too. The band
  // along the bottom stays with the player: its seek bar is dragged sideways.
  function onVideoControls(video: Element, y: number): boolean {
    const rect = video.getBoundingClientRect();
    const band = Math.max(VIDEO_CONTROLS_MIN_PX, rect.height * VIDEO_CONTROLS_SHARE);
    return y >= rect.bottom - band;
  }

  function ownsTouch(e: PointerEvent): boolean {
    const target = e.target;
    if (!(target instanceof Element)) return false;
    if (target.closest('.frame, button, a') !== null) return true;
    const video = target.closest('video');
    return video !== null && onVideoControls(video, e.clientY);
  }
  function onStageDown(e: PointerEvent): void {
    if (ownsTouch(e)) return;
    gesture = pointerDown(gesture, e.pointerId, center(e), STILL);
  }
  function onStageMove(e: PointerEvent): void {
    gesture = pointerMove(gesture, e.pointerId, center(e), LIMITS).state;
  }
  function onStageUp(e: PointerEvent): void {
    const ended = pointerUp(gesture, e.pointerId, center(e), STILL, Date.now());
    gesture = ended.state;
    if (ended.effect?.kind !== 'swipe') return;
    if (ended.effect.direction === 'next') forward();
    else back();
  }
  function onStageCancel(e: PointerEvent): void {
    gesture = pointerCancel(gesture, e.pointerId, STILL);
  }
</script>

<svelte:window onkeydown={onKeydown} />

<div
  class="viewer"
  bind:this={dialogEl}
  role="dialog"
  aria-modal="true"
  aria-label={`Day ${dayNumber}, ${seriesName}`}
  tabindex="-1"
>
  <!-- Not a <header>: that would be a second page banner beside Home's. -->
  <div class="top">
    <div class="meta">
      <span class="eyebrow">Day {dayNumber}</span>
      {#if dateText && postedAt !== undefined}
        <time datetime={isoInstant(postedAt)}>{dateText}</time>
      {/if}
    </div>
    {#if media.length > 1}
      <span class="pill">
        <span aria-hidden="true">{shown + 1} / {media.length}</span>
        <span class="sr-only">{FileWord} {shown + 1} of {media.length}</span>
      </span>
    {/if}
    <IconButton ariaLabel="Close" variant="solid" icon="close" onclick={() => onClose()} />
  </div>

  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="stage"
    onpointerdown={onStageDown}
    onpointermove={onStageMove}
    onpointerup={onStageUp}
    onpointercancel={onStageCancel}
  >
    {#if !day}
      {#if error}
        <div class="note" role="alert">
          <p class="note-title">Couldn’t load this day</p>
          <p>{error}</p>
          {#if onRetry}
            <Button size="sm" variant="primary" onclick={onRetry}>Try again</Button>
          {/if}
        </div>
      {:else}
        {#if summary?.thumb_url}
          <img class="preview" src={summary.thumb_url} alt="" aria-hidden="true" />
        {/if}
        <div class="waiting"><Spinner size="34px" label={`Loading Day ${dayNumber}`} /></div>
      {/if}
    {:else if !current}
      <div class="note"><p>This day has no photo or video.</p></div>
    {:else if current.missing}
      <div class="note">
        <p class="note-title">No file was saved for this day</p>
        <p>The original post may still have it. Use “Open original post” below.</p>
      </div>
    {:else if isVideo}
      {#if videoFailed}
        <div class="note" role="alert">
          <p class="note-title">This video didn’t play</p>
          {#if onRetry}
            <p>Try again, or use “Open original post” below to watch it in the channel.</p>
            <Button size="sm" variant="primary" onclick={retryVideo}>Try again</Button>
          {:else}
            <p>Use “Open original post” below to watch it in the channel.</p>
          {/if}
        </div>
      {:else}
        {#key `${videoAttempt} ${current.url}`}
          <!-- svelte-ignore a11y_media_has_caption -->
          <video
            class="media"
            src={current.url}
            poster={current.thumb_url || undefined}
            preload="metadata"
            controls
            playsinline
            onerror={() => (videoFailedUrl = current.url)}
          ></video>
        {/key}
      {/if}
    {:else}
      <ZoomableImage
        src={current.url}
        placeholder={current.thumb_url || undefined}
        {alt}
        onPrev={canBack ? back : undefined}
        onNext={canForward ? forward : undefined}
        {onRetry}
      />
    {/if}

    {#if canBack}
      <div class="edge left">
        <IconButton ariaLabel={backLabel} variant="overlay" onclick={back}>‹</IconButton>
      </div>
    {/if}
    {#if canForward}
      <div class="edge right">
        <IconButton ariaLabel={forwardLabel} variant="overlay" onclick={forward}>›</IconButton>
      </div>
    {/if}
  </div>

  <div class="bottom">
    {#if media.length > 1}
      <div class="dots">
        {#each dots as i (i)}
          <button
            type="button"
            class="dot"
            class:on={i === shown}
            aria-label={`${FileWord} ${i + 1} of ${media.length}`}
            aria-current={i === shown ? 'true' : undefined}
            onclick={() => (wanted = i)}
          ></button>
        {/each}
      </div>
    {/if}

    {#if caption}
      <div class="caption-block">
        <!-- Scrollable once expanded, so it has to be reachable by keyboard. -->
        <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
        <p class="caption" class:expanded bind:this={captionEl} tabindex={expanded ? 0 : undefined}>
          {caption}
        </p>
        {#if clipped || expanded}
          <button
            type="button"
            class="more"
            aria-expanded={expanded}
            onclick={() => (expandedDay = expanded ? null : dayNumber)}
          >
            {expanded ? 'Less' : 'More'}
          </button>
        {/if}
      </div>
    {/if}

    {#if linkFailed && day}
      <div class="notice" role="status">
        <p>Discord didn’t open the post. Copy this link and paste it into Discord or a browser:</p>
        <p class="url">{day.jump_url}</p>
        {#if canCopy}
          <Button size="sm" onclick={() => void copyLink()}>
            {copyState === 'copied' ? 'Copied' : 'Copy link'}
          </Button>
        {/if}
        {#if copyState === 'failed'}
          <p>Couldn’t copy it. Press and hold the link to select it.</p>
        {/if}
      </div>
    {/if}

    <div class="actions">
      <Button size="sm" disabled={!day} onclick={() => void openPost()}>Open original post</Button>
      {#if onRandom}
        <Button size="sm" onclick={random}>Random</Button>
      {/if}
    </div>
  </div>
</div>

<style>
  .viewer {
    position: absolute;
    inset: 0;
    display: grid;
    /* Not `auto`: a caption with no spaces would widen the whole viewer and
     * push Close off the screen. */
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: auto minmax(0, 1fr) auto;
    grid-template-areas: 'top' 'stage' 'bottom';
    min-height: 0;
    background: var(--canvas);
    outline: none;
  }

  /* The overlay is fixed, outside #app's padding: each edge keeps clear of
   * the notch and Discord's own controls itself. */
  .top {
    grid-area: top;
    display: flex;
    gap: var(--space-sm);
    align-items: center;
    min-width: 0;
    min-height: var(--appbar-h);
    padding: max(var(--space-xs), var(--safe-top)) max(var(--space-md), var(--safe-right))
      var(--space-xs) max(var(--space-md), var(--safe-left));
  }
  .meta {
    display: flex;
    flex: 1 1 0;
    flex-wrap: wrap;
    gap: 0 var(--space-sm);
    align-items: baseline;
    min-width: 0;
    color: var(--ink-muted);
  }
  .eyebrow {
    color: var(--ink);
    font-size: var(--fs-eyebrow);
    font-weight: var(--fw-emphasis);
    letter-spacing: 0.6px;
    text-transform: uppercase;
    white-space: nowrap;
  }
  time {
    font-size: var(--fs-caption);
    white-space: nowrap;
  }
  .pill {
    position: relative;
    flex: none;
    padding: 2px var(--space-sm);
    color: var(--ink);
    font-size: var(--fs-caption);
    font-weight: var(--fw-emphasis);
    font-variant-numeric: tabular-nums;
    background: var(--surface-1);
    border: 1px solid var(--control-border);
    border-radius: var(--radius-pill);
  }

  .stage {
    grid-area: stage;
    position: relative;
    display: flex;
    align-items: stretch;
    justify-content: stretch;
    min-width: 0;
    min-height: 0;
    padding: 0 max(var(--space-md), var(--safe-right)) 0 max(var(--space-md), var(--safe-left));
    /* Sideways swipes turn the page; the browser keeps vertical ones. */
    touch-action: pan-y;
  }
  .stage :global(.frame) {
    flex: 1;
    min-width: 0;
    min-height: 0;
  }
  .media {
    align-self: center;
    justify-self: center;
    max-width: 100%;
    max-height: 100%;
    margin: auto;
  }
  .preview {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: contain;
    filter: blur(14px);
    opacity: 0.7;
  }
  .waiting {
    position: relative;
    display: grid;
    flex: 1;
    place-items: center;
  }
  .note {
    display: grid;
    gap: var(--space-xs);
    align-self: center;
    justify-items: center;
    max-width: 24rem;
    margin: auto;
    /* Clear of the arrows on either side. */
    padding: 0 calc(var(--touch-target) + var(--space-xs));
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
    text-align: center;
  }
  .note p {
    margin: 0;
  }
  .note-title {
    color: var(--ink);
    font-size: var(--fs-body);
    font-weight: var(--fw-emphasis);
  }

  .edge {
    position: absolute;
    top: 50%;
    transform: translateY(-50%);
    z-index: 1;
  }
  .edge.left {
    left: max(var(--space-sm), var(--safe-left));
  }
  .edge.right {
    right: max(var(--space-sm), var(--safe-right));
  }

  .bottom {
    grid-area: bottom;
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-xs);
    justify-items: center;
    min-height: 0;
    padding: var(--space-xs) max(var(--space-md), var(--safe-right))
      max(var(--space-md), var(--safe-bottom)) max(var(--space-md), var(--safe-left));
  }

  /* Each dot is a full-height target, as wide as the row allows (44px for a
   * handful of files, never under 24px for ten); the mark inside is small. */
  .dots {
    display: flex;
    justify-content: center;
    width: 100%;
  }
  .dot {
    position: relative;
    flex: 0 1 var(--touch-target);
    min-width: 24px;
    height: var(--touch-target);
    padding: 0;
    background: none;
    border: 0;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .dot::before {
    content: '';
    position: absolute;
    top: 50%;
    left: 50%;
    width: 10px;
    height: 10px;
    background: var(--surface-1);
    border: 2px solid var(--control-border);
    border-radius: var(--radius-pill);
    transform: translate(-50%, -50%);
  }
  .dot.on::before {
    width: 12px;
    height: 12px;
    background: var(--ink);
    border-color: var(--ink);
  }
  .dot:focus-visible {
    outline-offset: -2px;
  }

  .caption-block {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    justify-items: center;
    max-width: 40rem;
  }
  .caption {
    /* Three lines until asked for more, so the photo keeps the screen. */
    display: -webkit-box;
    max-width: 100%;
    margin: 0;
    overflow: hidden;
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
    text-align: center;
    white-space: pre-line;
    /* `anywhere`, so a long link also stops counting towards the width this
     * centred block asks for. */
    overflow-wrap: anywhere;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    user-select: text;
  }
  .caption.expanded {
    display: block;
    max-height: 40vh;
    max-height: 40dvh;
    overflow-y: auto;
    overscroll-behavior: contain;
    -webkit-line-clamp: unset;
    line-clamp: unset;
  }
  .more {
    min-width: var(--touch-target);
    min-height: var(--touch-target);
    padding: 0 var(--space-sm);
    color: var(--link);
    font: inherit;
    font-size: var(--fs-body-sm);
    font-weight: var(--fw-emphasis);
    background: none;
    border: 0;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }

  .notice {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-xs);
    justify-items: center;
    max-width: 32rem;
    padding: var(--space-sm) var(--space-md);
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
    text-align: center;
    background: var(--surface-1);
    border: 1px solid var(--hairline);
    border-left: 2px solid var(--warning-fill);
    border-radius: var(--radius-lg);
  }
  .notice p {
    margin: 0;
  }
  .url {
    color: var(--ink);
    font-family: var(--font-mono);
    font-size: var(--fs-caption);
    overflow-wrap: anywhere;
    user-select: all;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-xs);
    justify-content: center;
  }

  /* A phone on its side: the photo takes the height, and everything else
   * moves to a column beside it. */
  @media (orientation: landscape) and (max-height: 520px) {
    .viewer {
      grid-template-columns: minmax(0, 1fr) minmax(200px, 34%);
      grid-template-rows: auto minmax(0, 1fr);
      grid-template-areas: 'stage top' 'stage bottom';
    }
    .top {
      padding-left: var(--space-xs);
    }
    .stage {
      padding: max(var(--space-xs), var(--safe-top)) var(--space-xs)
        max(var(--space-xs), var(--safe-bottom)) max(var(--space-md), var(--safe-left));
    }
    .edge.right {
      right: var(--space-sm);
    }
    .bottom {
      align-content: end;
      padding-left: var(--space-xs);
      overflow-y: auto;
    }
    /* The column is too narrow for ten dots in a row: they would spill out
     * of both sides, and the left spill cannot be scrolled to. */
    .dots {
      flex-wrap: wrap;
    }
    .dot {
      flex-basis: 24px;
    }
    .caption.expanded {
      max-height: none;
      overflow-y: visible;
    }
  }
</style>
