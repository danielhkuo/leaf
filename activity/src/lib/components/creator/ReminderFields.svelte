<script lang="ts">
  // The reminder part of the settings form: on or off, when, in which
  // timezone and how, with a plain sentence saying what will then happen.
  // Freeform series have no schedule, so the switch is shown off and locked
  // there instead of looking on while doing nothing.
  import { deviceTimezone } from '../../utils/timezones';
  import { reminderSummary } from './reminderCopy';
  import { reminderDefaults } from './settingsForm';
  import TimezoneSelect from './TimezoneSelect.svelte';

  type ReminderField = 'reminderEnabled' | 'reminderTime' | 'reminderTz' | 'reminderDm';

  interface Props {
    /** Prefix for the controls' ids: `<id>-reminderTime` and so on. */
    id: string;
    enabled: boolean;
    /** Whether reminders are on in the saved settings (never for freeform). */
    savedOn: boolean;
    /** `HH:MM`, or `''`. */
    time: string;
    /** An IANA zone, or `''` for the server's. */
    timezone: string;
    dm: boolean;
    cadence: string;
    /** The server's timezone, used when the series has none of its own. */
    serverZone: string;
    /** The series channel as shown (`#daily-sketch`); `null` when it has none. */
    channel: string | null;
    /** Not everyone can see the series (not public, or still a sprout). */
    hidden: boolean;
    /** Whether a day is archived yet, when known. */
    hasDays?: boolean | undefined;
    timeError: string | null;
    zoneError: string | null;
    onedit: (field: ReminderField) => void;
  }
  let {
    id,
    enabled = $bindable(),
    savedOn,
    time = $bindable(),
    timezone = $bindable(),
    dm = $bindable(),
    cadence,
    serverZone,
    channel,
    hidden,
    hasDays,
    timeError,
    zoneError,
    onedit,
  }: Props = $props();

  const device = deviceTimezone();
  const freeform = $derived(cadence === 'freeform');
  const on = $derived(enabled && !freeform);
  const where = $derived(channel ?? 'the series channel');
  const summary = $derived(
    reminderSummary({
      cadence,
      time,
      zone: timezone.trim() || serverZone,
      dm,
      channel,
      deviceZone: device,
      hasDays,
    }),
  );

  function toggle(e: Event & { currentTarget: HTMLInputElement }): void {
    enabled = e.currentTarget.checked;
    if (enabled) {
      const filled = reminderDefaults({ reminderTime: time, reminderTz: timezone }, device);
      time = filled.reminderTime;
      timezone = filled.reminderTz;
    }
    onedit('reminderEnabled');
  }

  function deliver(byDm: boolean): void {
    dm = byDm;
    onedit('reminderDm');
  }
</script>

<fieldset class="reminders">
  <legend>Reminders</legend>
  <label class="check">
    <input
      id="{id}-reminderEnabled"
      type="checkbox"
      checked={on}
      disabled={freeform}
      aria-describedby="{id}-reminder-about"
      onchange={toggle}
    />
    <span>Remind me when I’m behind</span>
  </label>

  {#if freeform}
    <p class="hint" id="{id}-reminder-about">
      Freeform series have no schedule, so there is nothing to remind about.
      {#if savedOn}Saving turns this series’ reminders off.{/if}
    </p>
  {:else if !on}
    <p class="hint" id="{id}-reminder-about">
      leaf can nudge you on a day that has no post archived yet. It stays off until you turn it on.
    </p>
  {:else}
    <div class="two">
      <div class="field">
        <label class="field-label" for="{id}-reminderTime">Time</label>
        <input
          id="{id}-reminderTime"
          class="control"
          type="time"
          required
          bind:value={time}
          aria-invalid={timeError ? 'true' : undefined}
          aria-describedby={timeError ? `${id}-reminderTime-error` : undefined}
          oninput={() => onedit('reminderTime')}
        />
        {#if timeError}<p class="field-error" id="{id}-reminderTime-error">{timeError}</p>{/if}
      </div>
      <div class="field">
        <label class="field-label" for="{id}-reminderTz">Timezone</label>
        <TimezoneSelect
          id="{id}-reminderTz"
          bind:value={timezone}
          {serverZone}
          invalid={zoneError !== null}
          describedby={zoneError ? `${id}-reminderTz-error` : undefined}
          onedit={() => onedit('reminderTz')}
        />
        {#if zoneError}<p class="field-error" id="{id}-reminderTz-error">{zoneError}</p>{/if}
      </div>
    </div>

    <fieldset class="how">
      <legend>How leaf nudges you</legend>
      <label class="check">
        <input type="radio" name="{id}-how" checked={dm} onchange={() => deliver(true)} />
        <span>A direct message</span>
      </label>
      <label class="check">
        <input type="radio" name="{id}-how" checked={!dm} onchange={() => deliver(false)} />
        <span>A ping in {where}</span>
      </label>
    </fieldset>
    {#if dm}
      <p class="hint">
        Discord only delivers it if you allow direct messages from members of this server. If a
        reminder can’t be delivered, this screen says so.
      </p>
    {:else if channel === null}
      <p class="hint caution">
        This series has no channel, so a ping has nowhere to go. Choose its channel above.
      </p>
    {:else if hidden}
      <p class="hint">
        The ping mentions you in {where} and says “your series”. It doesn’t name this series, because
        not everyone there can see it.
      </p>
    {/if}

    <p class="summary" id="{id}-reminder-about">{summary}</p>
  {/if}
</fieldset>

<style>
  fieldset {
    min-width: 0;
    margin: 0;
  }
  /* Single shrinkable columns: a long channel name wraps instead of
   * widening the form. */
  .reminders {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-sm);
    padding: var(--space-sm) var(--space-md) var(--space-md);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-lg);
  }
  .how {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    padding: 0;
    border: 0;
  }
  legend {
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
    font-weight: var(--fw-emphasis);
  }
  .reminders > legend {
    padding: 0 var(--space-xs);
  }
  .how > legend {
    padding: 0;
  }
  .check {
    display: flex;
    gap: var(--space-sm);
    align-items: center;
    min-height: var(--touch-target);
  }
  .check span {
    min-width: 0;
  }
  .two {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-md);
  }
  .control[aria-invalid='true'] {
    border-color: var(--error-fill);
  }
  .hint,
  .summary,
  .field-error {
    margin: 0;
    font-size: var(--fs-body-sm);
  }
  .hint {
    color: var(--ink-muted);
    font-size: var(--fs-caption);
  }
  .caution {
    padding-left: var(--space-sm);
    color: var(--ink);
    border-left: 2px solid var(--warning-fill);
  }
  .summary {
    padding: var(--space-sm);
    color: var(--ink);
    background: var(--surface-2);
    border-radius: var(--radius-md);
  }
  /* No time chosen yet: nothing to say. */
  .summary:empty {
    display: none;
  }
  .field-error {
    color: var(--error);
  }
  @media (min-width: 560px) {
    .two {
      grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    }
  }
</style>
