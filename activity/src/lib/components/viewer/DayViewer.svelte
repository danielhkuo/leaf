<script lang="ts">
  import type { Day } from '../../types/api';
  import { formatPostedAt } from '../../utils/datetime';
  import { clampIndex } from '../../utils/navigation';
  import Button from '../ui/Button.svelte';
  import IconButton from '../ui/IconButton.svelte';
  import ZoomableImage from './ZoomableImage.svelte';

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
  <div class="stage">
    {#if !current}
      <p class="note">No media for this day.</p>
    {:else if current.missing}
      <p class="note">This day’s media wasn’t captured.</p>
    {:else if isVideo}
      <!-- svelte-ignore a11y_media_has_caption -->
      <video class="media" src={current.url} poster={current.thumb_url} controls playsinline
      ></video>
    {:else}
      <ZoomableImage
        src={current.url}
        placeholder={current.thumb_url}
        {alt}
        onPrev={hasPrev ? onPrev : undefined}
        onNext={hasNext ? onNext : undefined}
      />
    {/if}

    {#if hasPrev}
      <div class="edge left">
        <IconButton ariaLabel="Previous day" variant="overlay" onclick={onPrev}>‹</IconButton>
      </div>
    {/if}
    {#if hasNext}
      <div class="edge right">
        <IconButton ariaLabel="Next day" variant="overlay" onclick={onNext}>›</IconButton>
      </div>
    {/if}
  </div>

  <header class="top">
    <div class="meta">
      <span class="eyebrow">Day {day.day}</span>
      <time>{formatPostedAt(day.posted_at)}</time>
    </div>
    <IconButton ariaLabel="Close" variant="ghost" onclick={onClose}>✕</IconButton>
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
      <Button variant="secondary" onclick={onJump}>Jump to message</Button>
      <Button variant="secondary" onclick={onRandom}>Random</Button>
      <Button variant="primary" onclick={onClose}>Close</Button>
    </div>
  </div>
</div>

<style>
  .viewer {
    position: absolute;
    inset: 0;
    background: var(--canvas);
    outline: none;
  }
  .stage {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    padding: calc(var(--appbar-h) + var(--space-md)) var(--space-md) 96px;
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
    z-index: 1;
  }
  .edge.left {
    left: var(--space-sm);
  }
  .edge.right {
    right: var(--space-sm);
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
    flex-wrap: wrap;
    gap: var(--space-xs);
    justify-content: center;
  }
</style>
