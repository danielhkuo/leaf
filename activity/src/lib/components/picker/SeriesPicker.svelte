<script lang="ts">
  import type { Eligibility, Series } from '../../types/api';
  import { violationMessage } from '../../utils/labels';
  import Callout from '../shared/Callout.svelte';
  import Button from '../ui/Button.svelte';
  import SeriesCard from './SeriesCard.svelte';

  interface Props {
    series: Series[];
    onSelect: (series: Series) => void;
    /** Whether the viewer can start a series here; `null` while unknown. */
    eligibility?: Eligibility | null;
    /** Load state for {@link eligibility}. */
    eligibilityStatus?: 'loading' | 'ready' | 'failed';
    /** Whether the viewer has series of their own here (revoked ones too). */
    ownsSeries?: boolean;
    /** Ids of the series that archive from the channel leaf was opened in. */
    inChannel?: readonly number[];
    onCreate?: () => void;
    onManage?: () => void;
  }
  let {
    series,
    onSelect,
    eligibility = null,
    eligibilityStatus = 'loading',
    ownsSeries = false,
    inChannel = [],
    onCreate,
    onManage,
  }: Props = $props();

  const canCreate = $derived(eligibility?.can_create ?? false);
  const blockers = $derived(eligibility && !eligibility.can_create ? eligibility.violations : []);
  /** Show the CTA when allowed, or when the check failed (create screen re-validates). */
  const showCreate = $derived(canCreate || eligibilityStatus === 'failed');
  // Nothing can be started or archived until an admin runs /setup, which is
  // the whole story for an empty server, not a personal restriction.
  const notSetUp = $derived(blockers.some((v) => v.code === 'guild_not_setup'));
  const reasons = $derived(
    blockers.filter((v) => v.code !== 'guild_not_setup').map((v) => violationMessage(v)),
  );

  const here = $derived(series.filter((s) => inChannel.includes(s.id)));
  const elsewhere = $derived(series.filter((s) => !inChannel.includes(s.id)));
  const grouped = $derived(here.length > 0 && elsewhere.length > 0);
  const uid = $props.id();
</script>

{#snippet list(items: Series[])}
  <ul class="list">
    {#each items as s (s.id)}
      <li><SeriesCard series={s} {onSelect} /></li>
    {/each}
  </ul>
{/snippet}

<div class="picker">
  <header class="head">
    <h1 tabindex="-1">Series</h1>
    {#if ownsSeries && onManage}
      <button type="button" class="link" onclick={onManage}>Manage my series</button>
    {/if}
  </header>

  {#if series.length === 0 && notSetUp}
    <Callout title="leaf isn’t set up in this server yet">
      A server admin needs to run /setup in chat. Series will show up here after that.
    </Callout>
  {:else if series.length === 0}
    <Callout title="No series here yet">
      {#if canCreate}
        Start the first one. You add its days from chat, and they show up here.
      {:else}
        Series people start in this server show up here.
      {/if}
    </Callout>
  {:else if grouped}
    <section class="group" aria-labelledby="{uid}-here">
      <h2 id="{uid}-here">In this channel</h2>
      {@render list(here)}
    </section>
    <section class="group" aria-labelledby="{uid}-more">
      <h2 id="{uid}-more">More series</h2>
      {@render list(elsewhere)}
    </section>
  {:else}
    {@render list(series)}
  {/if}

  {#if showCreate && onCreate}
    <Button variant="primary" full disabled={eligibilityStatus === 'loading'} onclick={onCreate}>
      {eligibilityStatus === 'loading' ? 'Checking…' : 'Start a series'}
    </Button>
  {:else if reasons.length > 0}
    <!-- Most people here only browse: the reason waits behind a quiet
         question instead of standing on the screen as a refusal. -->
    <details class="why">
      <summary>Want your own series?</summary>
      {#if reasons.length === 1}
        <p>{reasons[0]}</p>
      {:else}
        <ul>
          {#each reasons as reason (reason)}
            <li>{reason}</li>
          {/each}
        </ul>
      {/if}
    </details>
  {/if}
</div>

<style>
  .picker {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-md);
    width: 100%;
    max-width: 40rem;
    margin: 0 auto;
    padding: var(--space-lg);
  }
  .head {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-xs);
    align-items: center;
    justify-content: space-between;
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
  h2 {
    margin: 0 0 var(--space-xs);
    color: var(--ink-subtle);
    font-family: inherit;
    font-size: var(--fs-eyebrow);
    font-weight: var(--fw-emphasis);
    letter-spacing: 0.6px;
    text-transform: uppercase;
  }
  .link {
    display: inline-flex;
    align-items: center;
    min-height: var(--touch-target);
    margin-right: calc(-1 * var(--space-xs));
    padding: 0 var(--space-xs);
    color: var(--link);
    font: inherit;
    font-size: var(--fs-body-sm);
    font-weight: var(--fw-emphasis);
    background: none;
    border: 0;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .list {
    display: grid;
    gap: var(--space-sm);
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .why {
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
  }
  .why summary {
    width: fit-content;
    padding: 0 var(--space-xs);
    margin-left: calc(-1 * var(--space-xs));
    color: var(--link);
    font-weight: var(--fw-emphasis);
    line-height: var(--touch-target);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .why p,
  .why ul {
    margin: var(--space-xxs) 0 0;
  }
  .why ul {
    padding-left: var(--space-md);
  }
  .why li + li {
    margin-top: var(--space-xxs);
  }
  @media (hover: hover) {
    .link:hover,
    .why summary:hover {
      text-decoration: underline;
    }
  }
</style>
