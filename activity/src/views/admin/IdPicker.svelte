<script lang="ts" module>
  /** One role or channel a picker offers. */
  export interface Choice {
    id: string;
    label: string;
  }
</script>

<script lang="ts">
  // A role or channel, picked by name. Discord ids are 18-digit numbers nobody
  // should have to copy, so the list from the server is the normal control.
  // When leaf could not get the list (an older server, Discord not answering)
  // the id can still be pasted into a text box, so a save is never blocked.
  interface Props {
    id: string;
    /** `undefined` while the list loads; `null` when leaf could not get it. */
    choices: Choice[] | null | undefined;
    /** The chosen id; `''` for none. */
    value: string;
    /** The saved id. It stays on offer even when Discord no longer lists it. */
    stored?: string;
    /** Label of the "none" option. Leave out where a choice is required. */
    noneLabel?: string | undefined;
    /** Shown before a required choice is made. */
    placeholder?: string;
    /** What to call an id that is not in the list. */
    unlisted: (id: string) => string;
    /** The text box's placeholder, e.g. "Role ID". */
    idHint: string;
    invalid?: boolean;
    describedby?: string | undefined;
    disabled?: boolean;
    onedit?: (() => void) | undefined;
  }
  let {
    id,
    choices,
    value = $bindable(),
    stored = '',
    noneLabel,
    placeholder = 'Choose…',
    unlisted,
    idHint,
    invalid = false,
    describedby,
    disabled = false,
    onedit,
  }: Props = $props();

  /** Ids to offer that the list lacks: the saved one, and whatever is chosen now. */
  const extras = $derived(
    [...new Set([stored, value])].filter(
      (extra) => extra !== '' && !(choices ?? []).some((choice) => choice.id === extra),
    ),
  );
</script>

{#if choices === undefined}
  <!-- Same size as the real control, so nothing moves when the list arrives. -->
  <select {id} class="control" disabled aria-describedby={describedby}>
    <option>Loading…</option>
  </select>
{:else if choices === null}
  <input
    {id}
    class="control"
    type="text"
    inputmode="numeric"
    pattern="[0-9]*"
    bind:value
    placeholder={idHint}
    autocomplete="off"
    spellcheck="false"
    enterkeyhint="done"
    {disabled}
    aria-invalid={invalid ? 'true' : undefined}
    aria-describedby={describedby}
    oninput={onedit}
  />
{:else}
  <select
    {id}
    class="control"
    bind:value
    {disabled}
    aria-invalid={invalid ? 'true' : undefined}
    aria-describedby={describedby}
    onchange={onedit}
  >
    {#if noneLabel !== undefined}
      <option value="">{noneLabel}</option>
    {:else if value === ''}
      <option value="" disabled>{placeholder}</option>
    {/if}
    {#each extras as extra (extra)}
      <option value={extra}>{unlisted(extra)}</option>
    {/each}
    {#each choices as choice (choice.id)}
      <option value={choice.id}>{choice.label}</option>
    {/each}
  </select>
{/if}

<style>
  .control[aria-invalid='true'] {
    border-color: var(--error-fill);
  }
</style>
