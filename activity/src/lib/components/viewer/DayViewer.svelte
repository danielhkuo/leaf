<script lang="ts">
  import type { Day } from '../../types/api';
  import { formatPostedAt } from '../../utils/datetime';
  import { clampIndex } from '../../utils/navigation';
  import BlurImage from '../shared/BlurImage.svelte';

  interface Props {
    day: Day;
    seriesName: string;
    hasPrev: boolean;
    hasNext: boolean;
    onPrev: () => void;
    onNext: () => void;
    onRandom: () => void;
    onClose: () => void;
    onJump: () => void;
  }
  let { day, seriesName, hasPrev, hasNext, onPrev, onNext, onRandom, onClose, onJump }: Props =
    $props();

  let attachmentIndex = $state(0);
  // Reset the carousel when the day changes.
  $effect(() => {
    void day.day;
    attachmentIndex = 0;
  });

  const dots = $derived(day.media.map((_, i) => i));
  const current = $derived(day.media[clampIndex(attachmentIndex, day.media.length)]);
  const isVideo = $derived(current?.content_type.startsWith('video/') ?? false);
  const caption = $derived(day.caption.trim());
  const alt = $derived(caption || `Day ${day.day} of ${seriesName}`);

  // Manage focus: trap into the dialog, restore on close.
  let dialogEl: HTMLElement | undefined;
  $effect(() => {
    const previous = document.activeElement;
    dialogEl?.focus();
    return () => {
      if (previous instanceof HTMLElement) previous.focus();
    };
  });

  function onKeydown(e: KeyboardEvent): void {
    if (e.key === 'ArrowRight' && hasNext) onNext();
    else if (e.key === 'ArrowLeft' && hasPrev) onPrev();
    else if (e.key === 'Escape') onClose();
  }

  // Swipe via pointer events (no scroll math).
  const SWIPE = 50;
  let startX = 0;
  let tracking = false;
  function onPointerDown(e: PointerEvent): void {
    tracking = true;
    startX = e.clientX;
  }
  function onPointerUp(e: PointerEvent): void {
    if (!tracking) return;
    tracking = false;
    const dx = e.clientX - startX;
    if (dx <= -SWIPE && hasNext) onNext();
    else if (dx >= SWIPE && hasPrev) onPrev();
  }
</script>

<svelte:window onkeydown={onKeydown} />

<div
  class="viewer"
  bind:this={dialogEl}
  role="dialog"
  aria-modal="true"
  aria-label={`Day ${day.day}, ${seriesName}`}
  tabindex="-1"
>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="stage" onpointerdown={onPointerDown} onpointerup={onPointerUp}>
    {#if !current}
      <p class="note">No media for this day.</p>
    {:else if current.missing}
      <p class="note">This day’s media wasn’t captured.</p>
    {:else if isVideo}
      <!-- svelte-ignore a11y_media_has_caption -->
      <video class="media" src={current.url} poster={current.thumb_url} controls playsinline
      ></video>
    {:else}
      <BlurImage src={current.url} placeholder={current.thumb_url} {alt} />
    {/if}

    {#if hasPrev}
      <button class="edge left" aria-label="Previous day" onclick={onPrev}>‹</button>
    {/if}
    {#if hasNext}
      <button class="edge right" aria-label="Next day" onclick={onNext}>›</button>
    {/if}
  </div>

  <header class="top">
    <div class="meta">
      <span class="eyebrow">Day {day.day}</span>
      <time>{formatPostedAt(day.posted_at)}</time>
    </div>
    <button class="icon" aria-label="Close" onclick={onClose}>✕</button>
  </header>

  <div class="bottom">
    {#if dots.length > 1}
      <div class="dots">
        {#each dots as i (i)}
          <button
            class="dot"
            class:on={i === attachmentIndex}
            aria-label={`Attachment ${i + 1} of ${dots.length}`}
            aria-current={i === attachmentIndex}
            onclick={() => (attachmentIndex = i)}
          ></button>
        {/each}
      </div>
    {/if}

    {#if caption}
      <p class="caption">{caption}</p>
    {/if}

    <div class="actions">
      <button class="action" onclick={onJump}>Jump to message</button>
      <button class="action" onclick={onRandom}>Random</button>
      <button class="action primary" onclick={onClose}>Close</button>
    </div>
  </div>
</div>

<style>
  .viewer {
    position: absolute;
    inset: 0;
    display: grid;
    grid-template-rows: 1fr;
    background: var(--canvas);
    outline: none;
  }
  .stage {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    padding: calc(var(--appbar-h) + var(--space-md)) var(--space-md) 96px;
    touch-action: pan-y;
  }
  .media {
    max-width: 100%;
    max-height: 100%;
  }
  .note {
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
  }

  .edge {
    position: absolute;
    top: 50%;
    transform: translateY(-50%);
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    color: var(--ink);
    font-size: 1.5rem;
    line-height: 1;
    background: rgb(0 0 0 / 35%);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-pill);
    cursor: pointer;
  }
  .edge.left {
    left: var(--space-sm);
  }
  .edge.right {
    right: var(--space-sm);
  }
  .edge:active {
    background: rgb(0 0 0 / 55%);
  }

  .top {
    position: absolute;
    top: 0;
    right: 0;
    left: 0;
    display: flex;
    align-items: center;
    justify-content: space-between;
    min-height: var(--appbar-h);
    padding: var(--space-xs) var(--space-md);
    background: linear-gradient(var(--canvas), transparent);
  }
  .meta {
    display: flex;
    gap: var(--space-sm);
    align-items: baseline;
    color: var(--ink-muted);
  }
  .eyebrow {
    color: var(--ink);
    font-size: var(--fs-eyebrow);
    font-weight: var(--fw-emphasis);
    letter-spacing: 0.6px;
    text-transform: uppercase;
  }
  time {
    font-size: var(--fs-caption);
  }
  .icon {
    width: 36px;
    height: 36px;
    color: var(--ink);
    font: inherit;
    background: transparent;
    border: 0;
    border-radius: var(--radius-md);
    cursor: pointer;
  }
  .icon:active {
    background: var(--surface-2);
  }

  .bottom {
    position: absolute;
    right: 0;
    bottom: 0;
    left: 0;
    display: grid;
    gap: var(--space-sm);
    justify-items: center;
    padding: var(--space-md);
    padding-bottom: max(var(--space-md), env(safe-area-inset-bottom));
    background: linear-gradient(transparent, var(--canvas));
  }
  .dots {
    display: flex;
    gap: var(--space-xs);
  }
  .dot {
    width: 8px;
    height: 8px;
    padding: 0;
    background: var(--surface-3);
    border: 0;
    border-radius: var(--radius-pill);
    cursor: pointer;
  }
  .dot.on {
    background: var(--accent);
  }
  .caption {
    max-width: 40rem;
    margin: 0;
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
    text-align: center;
  }
  .actions {
    display: flex;
    gap: var(--space-xs);
  }
  .action {
    padding: 10px 14px;
    color: var(--ink);
    font: inherit;
    font-size: var(--fs-body-sm);
    font-weight: var(--fw-emphasis);
    background: var(--surface-2);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-md);
    cursor: pointer;
  }
  .action:active {
    background: var(--surface-3);
  }
  .action.primary {
    color: var(--on-accent-dark);
    background: var(--accent);
    border-color: transparent;
  }
</style>
