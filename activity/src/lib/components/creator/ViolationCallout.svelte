<script lang="ts">
  // Shows why a creator can't start (or edit) a series: the server's policy
  // messages, verbatim, so the rules match what the bot would say.
  import type { Violation } from '../../types/api';
  import Callout from '../shared/Callout.svelte';

  interface Props {
    title?: string;
    violations: Violation[];
  }
  let { title = 'You can’t start a series here yet', violations }: Props = $props();
</script>

<Callout {title}>
  {#if violations.length === 1}
    {violations[0]?.message}
  {:else}
    <ul>
      {#each violations as v (v.code)}
        <li>{v.message}</li>
      {/each}
    </ul>
  {/if}
</Callout>

<style>
  ul {
    margin: 0;
    padding-left: var(--space-md);
  }
  li + li {
    margin-top: var(--space-xxs);
  }
</style>
