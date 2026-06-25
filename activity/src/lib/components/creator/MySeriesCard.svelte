<script lang="ts">
  // One owned series in the "my series" list: identity, lifecycle state, and
  // a quick read of cadence / channel / archived days. Opens its settings.
  import type { MySeries } from '../../types/api';
  import { accentVar } from '../../utils/accent';
  import Card from '../ui/Card.svelte';

  interface Props {
    series: MySeries;
    onOpen: (series: MySeries) => void;
  }
  let { series, onOpen }: Props = $props();

  const accent = $derived(accentVar(series.id));
  const stateLabel = $derived(
    series.state === 'sprout' ? '🌱 sprout' : series.state === 'revoked' ? 'revoked' : null,
  );
</script>

<button class="open" onclick={() => onOpen(series)} style="--accent:{accent}">
  <Card accent>
    <div class="row">
      <span class="emoji" aria-hidden="true">{series.emoji}</span>
      <div class="meta">
        <span class="name">
          {series.name}
          {#if stateLabel}<span class="badge">{stateLabel}</span>{/if}
        </span>
        <span class="sub">
          {series.cadence}
          {#if series.channel_name}· #{series.channel_name}{/if}
          · {series.archived_days} archived
          {#if series.reminder_enabled}· reminders on{/if}
        </span>
      </div>
      <span class="chev" aria-hidden="true">⚙</span>
    </div>
  </Card>
</button>

<style>
  .open {
    display: block;
    width: 100%;
    padding: 0;
    font: inherit;
    text-align: left;
    background: none;
    border: 0;
    cursor: pointer;
  }
  .open:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
    border-radius: var(--radius-xl);
  }
  .row {
    display: grid;
    grid-template-columns: auto 1fr auto;
    gap: var(--space-sm);
    align-items: center;
  }
  .emoji {
    font-size: 1.5rem;
  }
  .meta {
    display: grid;
    gap: 2px;
    min-width: 0;
  }
  .name {
    display: flex;
    gap: var(--space-xs);
    align-items: center;
    font-weight: var(--fw-emphasis);
  }
  .badge {
    padding: 1px 8px;
    color: var(--ink-muted);
    font-size: var(--fs-eyebrow);
    background: var(--surface-2);
    border-radius: var(--radius-pill);
  }
  .sub {
    color: var(--ink-subtle);
    font-size: var(--fs-caption);
  }
  .chev {
    color: var(--ink-subtle);
    font-size: 1.1rem;
  }
</style>
