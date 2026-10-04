<script lang="ts">
  // Why a member can't start a series here. Each reason is the sentence
  // labels.ts holds for the server's code (the same copy the picker and a
  // refused submit use), so it names the role, the date or the next step. A
  // server where /setup has not been run is not a personal restriction, so it
  // gets its own title and nothing else.
  import type { Violation } from '../../types/api';
  import { violationMessage } from '../../utils/labels';
  import Callout from '../shared/Callout.svelte';

  interface Props {
    title?: string;
    violations: Violation[];
  }
  let { title = 'You can’t start a series here yet', violations }: Props = $props();

  const notSetUp = $derived(violations.some((v) => v.code === 'guild_not_setup'));
  const reasons = $derived(
    violations.length === 0
      ? ['Ask a server admin why.']
      : [...new Set(violations.map((v) => violationMessage(v)))],
  );
</script>

{#if notSetUp}
  <Callout title="leaf isn’t set up in this server yet">
    A server admin needs to run /setup in chat first. After that you can start a series here.
  </Callout>
{:else}
  <Callout {title}>
    {#if reasons.length === 1}
      {reasons[0]}
    {:else}
      <ul>
        {#each reasons as reason (reason)}
          <li>{reason}</li>
        {/each}
      </ul>
    {/if}
  </Callout>
{/if}

<style>
  ul {
    margin: 0;
    padding-left: var(--space-md);
  }
  li + li {
    margin-top: var(--space-xxs);
  }
</style>
