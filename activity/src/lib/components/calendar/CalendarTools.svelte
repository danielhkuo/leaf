<script lang="ts">
  // Ways into a long archive without scrolling it: the newest day, a random
  // one, a day by number, a month by name. Plus a refresh, since nothing
  // tells an open gallery that a day was archived in chat a minute ago.
  import { nearestDay } from '../../utils/calendar';
  import { randomDay } from '../../utils/navigation';
  import Button from '../ui/Button.svelte';
  import IconButton from '../ui/IconButton.svelte';

  interface Props {
    /** Archived day numbers, any order. */
    days: readonly number[];
    /** Months that have days, newest first. */
    months: readonly { key: string; label: string }[];
    refreshing: boolean;
    onOpenDay: (day: number) => void;
    onJumpMonth: (key: string) => void;
    onRefresh: () => void;
  }
  let { days, months, refreshing, onOpenDay, onJumpMonth, onRefresh }: Props = $props();

  const uid = $props.id();
  // A loop, not Math.max(...days): spreading a long archive can overflow the
  // argument limit.
  const latest = $derived(
    days.reduce<number | null>((max, d) => (max === null || d > max ? d : max), null),
  );

  let dayText = $state('');
  let note = $state('');

  function goToDay(e: SubmitEvent): void {
    e.preventDefault();
    // "42", "Day 42" and "#42" all mean 42; two numbers mean nothing.
    const runs = dayText.match(/\d+/g) ?? [];
    const wanted = runs.length === 1 ? Number(runs[0]) : Number.NaN;
    if (!Number.isInteger(wanted) || wanted < 1) {
      note = 'Enter one day number, such as 42.';
      return;
    }
    const found = nearestDay(days, wanted);
    if (found === null) return;
    note =
      found === wanted ? '' : `Day ${wanted} isn’t archived, so Day ${found}, the closest, opened.`;
    onOpenDay(found);
  }

  function jump(e: Event & { currentTarget: HTMLSelectElement }): void {
    const select = e.currentTarget;
    const key = select.value;
    // Back to the prompt, so choosing the same month again still jumps.
    select.value = '';
    if (key) onJumpMonth(key);
  }

  function random(): void {
    const day = randomDay(days, Number.NaN);
    if (day !== null) onOpenDay(day);
  }
</script>

<div>
  <div class="tools">
    <div class="quick">
      {#if latest !== null}
        <Button size="sm" variant="secondary" onclick={() => onOpenDay(latest)}>
          Latest: Day {latest}
        </Button>
      {/if}
      <Button
        size="sm"
        variant="secondary"
        ariaLabel="Random day"
        disabled={days.length === 0}
        onclick={random}
      >
        Random
      </Button>
      <span class="refresh">
        <IconButton
          ariaLabel={refreshing ? 'Refreshing' : 'Refresh'}
          icon="refresh"
          disabled={refreshing}
          onclick={onRefresh}
        />
      </span>
    </div>
    <div class="jump">
      <form class="goto" onsubmit={goToDay}>
        <label class="sr-only" for="{uid}-day">Go to day number</label>
        <input
          id="{uid}-day"
          class="control"
          type="text"
          inputmode="numeric"
          enterkeyhint="go"
          autocomplete="off"
          placeholder="Go to day"
          bind:value={dayText}
          oninput={() => (note = '')}
        />
        <Button size="sm" type="submit" disabled={days.length === 0}>Go</Button>
      </form>
      {#if months.length > 1}
        <select class="control month" aria-label="Jump to month" onchange={jump}>
          <option value="" selected disabled>Jump to month</option>
          {#each months as m (m.key)}
            <option value={m.key}>{m.label}</option>
          {/each}
        </select>
      {/if}
    </div>
  </div>
  <p class="note" role="status">{note}</p>
</div>

<style>
  .tools {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-xs) var(--space-md);
    align-items: center;
  }
  .quick {
    display: flex;
    flex: 1 1 auto;
    flex-wrap: wrap;
    gap: var(--space-xs);
    align-items: center;
  }
  .refresh {
    display: contents;
  }
  .refresh :global(button) {
    margin-left: auto;
  }
  .jump {
    display: grid;
    flex: 1 1 18rem;
    /* Side by side once each half has room for its text; stacked below. */
    grid-template-columns: repeat(auto-fit, minmax(9.5rem, 1fr));
    gap: var(--space-xs);
  }
  .goto {
    display: flex;
    gap: var(--space-xxs);
    min-width: 0;
  }
  .goto input {
    flex: 1 1 auto;
    min-width: 0;
    min-height: var(--touch-target);
    padding: 0 var(--space-sm);
    font-variant-numeric: tabular-nums;
  }
  .month {
    min-width: 0;
    min-height: var(--touch-target);
    padding: 0 var(--space-sm);
    text-overflow: ellipsis;
  }
  .note {
    margin: 0;
    color: var(--ink-muted);
    font-size: var(--fs-caption);
  }
  .note:not(:empty) {
    margin-top: var(--space-xs);
  }
</style>
