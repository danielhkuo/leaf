<script lang="ts">
  // Mirrors views/Home.svelte's composition (header + stats + calendar) with
  // fixture data, since the real view reads from the gallery store/API.
  import Calendar from '../lib/components/calendar/Calendar.svelte';
  import Callout from '../lib/components/shared/Callout.svelte';
  import StatsPanel from '../lib/components/stats/Stats.svelte';
  import Button from '../lib/components/ui/Button.svelte';
  import IconButton from '../lib/components/ui/IconButton.svelte';
  import { accentVar } from '../lib/utils/accent';
  import { dayIndex, homeSeries, stats } from './fixtures';

  interface Props {
    /** Show the "still sprouting" empty state instead of the calendar. */
    empty?: boolean;
  }
  let { empty = false }: Props = $props();

  const accent = accentVar(homeSeries.id);
  const noop = (): void => undefined;
</script>

<div class="home" style="--accent:{accent}">
  <header class="bar">
    <IconButton ariaLabel="Back to series list" variant="solid" onclick={noop}>←</IconButton>
    <span class="emoji" aria-hidden="true">{homeSeries.emoji}</span>
    <h1>{homeSeries.name}</h1>
    <span class="spacer"></span>
    <Button variant="secondary" onclick={noop}>Start a series</Button>
    <IconButton ariaLabel="Series settings" variant="solid" onclick={noop}>⚙</IconButton>
  </header>

  {#if empty}
    <Callout title="Still sprouting">
      No days archived yet — they’ll appear here as they’re posted.
    </Callout>
  {:else}
    <div class="content">
      <aside class="side"><StatsPanel {stats} /></aside>
      <main class="main"><Calendar index={dayIndex} onOpenDay={noop} /></main>
    </div>
  {/if}
</div>

<style>
  .home {
    display: grid;
    gap: var(--space-md);
    width: 100%;
    max-width: 64rem;
    margin: 0 auto;
    padding: var(--space-md);
  }
  .bar {
    position: sticky;
    top: 0;
    z-index: 1;
    display: flex;
    gap: var(--space-sm);
    align-items: center;
    min-height: var(--appbar-h);
    background: var(--canvas);
  }
  .emoji {
    font-size: 1.5rem;
  }
  .spacer {
    flex: 1;
  }
  h1 {
    margin: 0;
    font-size: var(--fs-card-title);
    font-weight: var(--fw-display);
    letter-spacing: var(--tracking-display);
  }
  .content {
    display: grid;
    gap: var(--space-lg);
    grid-template-areas: 'side' 'main';
  }
  .side {
    grid-area: side;
  }
  .main {
    grid-area: main;
    min-width: 0;
  }
  @media (min-width: 960px) {
    .content {
      grid-template-columns: 1fr 16rem;
      grid-template-areas: 'main side';
      align-items: start;
    }
    .side {
      position: sticky;
      top: calc(var(--appbar-h) + var(--space-md));
    }
  }
</style>
