<script lang="ts">
  // The server's timezone. Discord tells leaf nothing about where a server
  // is, and typing "America/Chicago" on a phone goes wrong, so the device's
  // own zone is one tap away and the full IANA list is a select. A browser
  // too old to list zones gets a text box instead.
  import { deviceTimezone, hasTimezoneList, timezoneOptions } from '../../lib/utils/timezones';

  interface Props {
    id: string;
    /** An IANA zone name. */
    value: string;
    /** The saved zone: always on offer, even when the browser's list lacks it. */
    stored: string;
    invalid?: boolean;
    describedby?: string | undefined;
    onedit?: (() => void) | undefined;
  }
  let { id, value = $bindable(), stored, invalid = false, describedby, onedit }: Props = $props();

  const device = deviceTimezone();
  const listed = hasTimezoneList();

  const zones = $derived(timezoneOptions(stored).filter((zone) => zone !== device));
  /** A value from a kept draft that the list does not hold still has to show. */
  const extra = $derived(value !== '' && value !== device && !zones.includes(value) ? value : null);
  const onDevice = $derived(device !== null && value.trim().toLowerCase() === device.toLowerCase());

  function useDevice(): void {
    if (device === null) return;
    value = device;
    onedit?.();
  }
</script>

<div class="zone">
  {#if listed}
    <select
      {id}
      class="control"
      bind:value
      aria-invalid={invalid ? 'true' : undefined}
      aria-describedby={describedby}
      onchange={onedit}
    >
      {#if extra}<option value={extra}>{extra}</option>{/if}
      <!-- The zone comes first: a phone's closed select cuts long text off. -->
      {#if device}<option value={device}>{device} (this device)</option>{/if}
      <optgroup label="All timezones">
        {#each zones as zone (zone)}
          <option value={zone}>{zone}</option>
        {/each}
      </optgroup>
    </select>
  {:else}
    <input
      {id}
      class="control"
      type="text"
      bind:value
      placeholder="Europe/Berlin"
      autocomplete="off"
      autocapitalize="off"
      autocorrect="off"
      spellcheck="false"
      enterkeyhint="done"
      aria-invalid={invalid ? 'true' : undefined}
      aria-describedby={describedby}
      oninput={onedit}
    />
  {/if}
  {#if device && !onDevice}
    <button type="button" class="link" onclick={useDevice}>Use my timezone ({device})</button>
  {/if}
</div>

<style>
  .zone {
    display: grid;
    justify-items: start;
  }
  .control[aria-invalid='true'] {
    border-color: var(--error-fill);
  }
  .link {
    min-height: var(--touch-target);
    margin-left: calc(-1 * var(--space-xs));
    padding: 0 var(--space-xs);
    color: var(--link);
    font: inherit;
    font-weight: var(--fw-emphasis);
    text-align: left;
    background: none;
    border: 0;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  @media (hover: hover) {
    .link:hover {
      text-decoration: underline;
    }
  }
</style>
