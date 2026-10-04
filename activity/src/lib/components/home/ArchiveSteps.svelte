<script lang="ts">
  // How a creator gets a post into their series. Archiving happens in chat,
  // from the message's own menu ("Archive to Series", the command's exact
  // registered name), which nothing in Discord points to, so the gallery
  // spells it out, one tap to a step. The menu is reached differently on
  // phones and desktop: a phone lists Apps low in the message menu and opens
  // a list of applications first, so the steps name this one as Discord does.
  import type { Platform } from '../../sdk/types';

  interface Props {
    platform: Platform;
    /**
     * The series' channel name, without `#`. `null` when it is not known (not
     * loaded yet, or the channel is gone or hidden from leaf): the step then
     * names no channel, since any of the server's series channels will do.
     */
    channelName: string | null;
    /** What this application is called in Discord's Apps list. */
    appName?: string | undefined;
  }
  let { platform, channelName, appName = 'leaf' }: Props = $props();

  const mobile = $derived(platform === 'mobile');
</script>

<ol class="steps">
  <li>
    {mobile ? 'Minimise leaf, then post' : 'Post'} your photo or video in
    {#if channelName}<strong>#{channelName}</strong>.{:else}one of this server’s series channels.{/if}
  </li>
  {#if mobile}
    <li>Press and hold your message.</li>
    <li>Tap <strong>Apps</strong>. Scroll down in the menu to find it.</li>
    <li>Choose <strong>{appName}</strong>, then <strong>Archive to Series</strong>.</li>
  {:else}
    <li>
      Right-click your message, choose <strong>Apps</strong>, then
      <strong>Archive to Series</strong>.
    </li>
  {/if}
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
