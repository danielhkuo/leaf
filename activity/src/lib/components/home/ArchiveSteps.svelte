<script lang="ts">
  // How a creator gets a post into their series. Archiving happens in chat,
  // from the message's own menu ("Archive to Series", the command's exact
  // registered name), which nothing in Discord points to, so the gallery
  // spells it out. The menu is reached differently on phones and desktop.
  import type { Platform } from '../../sdk/types';

  interface Props {
    platform: Platform;
    /** The series' channel name, without `#`; `null` when unknown. */
    channelName: string | null;
  }
  let { platform, channelName }: Props = $props();

  const mobile = $derived(platform === 'mobile');
</script>

<ol class="steps">
  <li>
    {mobile ? 'Minimise leaf, then post' : 'Post'} your photo or video in
    {#if channelName}<strong>#{channelName}</strong>.{:else}the channel you chose for this series.{/if}
  </li>
  <li>
    {#if mobile}
      Press and hold your message, tap <strong>Apps</strong>, then
      <strong>Archive to Series</strong>.
    {:else}
      Right-click your message, choose <strong>Apps</strong>, then
      <strong>Archive to Series</strong>.
    {/if}
  </li>
  <li>Check the day number leaf suggests and confirm it. The day shows up here.</li>
</ol>

<style>
  .steps {
    display: grid;
    gap: var(--space-xs);
    margin: 0;
    padding-left: var(--space-lg);
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
  }
  .steps li::marker {
    color: var(--ink);
    font-weight: var(--fw-emphasis);
  }
  strong {
    color: var(--ink);
    font-weight: var(--fw-emphasis);
    overflow-wrap: anywhere;
  }
</style>
