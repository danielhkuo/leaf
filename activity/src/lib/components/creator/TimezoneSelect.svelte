<script lang="ts">
  // The reminder's timezone. Discord tells leaf nothing about where a person
  // is, so the choices that matter come first: the server's timezone (the
  // default, sent as "") and the device's own. The full IANA list follows for
  // everyone else. A webview too old to list zones gets a text box instead.
  import { deviceTimezone, hasTimezoneList, timezoneOptions } from '../../utils/timezones';

  interface Props {
    id: string;
    /** An IANA zone name, or `''` for the server's timezone. */
    value: string;
    /** The server's timezone: what `''` means. */
    serverZone: string;
    invalid?: boolean;
    describedby?: string | undefined;
    onedit?: () => void;
  }
  let {
    id,
    value = $bindable(),
    serverZone,
    invalid = false,
    describedby,
    onedit,
  }: Props = $props();

  /** Select value that reveals the text box (never a zone name). */
  const OTHER = '__other__';

  const device = deviceTimezone();
  const listed = hasTimezoneList();
  // Built once: it holds the saved zone even when the webview's list lacks it.
  const zones = timezoneOptions(value || null).filter((zone) => zone !== device);

  // Typing a zone by hand: offered only where there is no list to pick from.
  let typing = $state(false);

  function choose(picked: string): void {
    typing = picked === OTHER;
    value = typing ? '' : picked;
    onedit?.();
  }
</script>

<div class="zone">
  <select
    {id}
    class="control"
    bind:value={() => (typing ? OTHER : value), choose}
    aria-invalid={invalid && !typing ? 'true' : undefined}
    aria-describedby={describedby}
  >
    <!-- The zone comes first: a phone's closed select cuts long text off. -->
    <option value="">{serverZone} (server default)</option>
    {#if device}<option value={device}>{device} (this device)</option>{/if}
    <optgroup label={listed ? 'All timezones' : 'Other timezones'}>
      {#each zones as zone (zone)}
        <option value={zone}>{zone}</option>
      {/each}
      {#if !listed}<option value={OTHER}>Another timezone…</option>{/if}
    </optgroup>
  </select>
  {#if typing}
    <input
      class="control"
      bind:value
      placeholder="Europe/Berlin"
      aria-label="Timezone name, such as Europe/Berlin"
      aria-invalid={invalid ? 'true' : undefined}
      aria-describedby={describedby}
      autocomplete="off"
      autocapitalize="off"
      autocorrect="off"
      spellcheck="false"
      oninput={onedit}
    />
  {/if}
</div>

<style>
  .zone {
    display: grid;
    gap: var(--space-xs);
  }
  .control[aria-invalid='true'] {
    border-color: var(--error-fill);
  }
</style>
