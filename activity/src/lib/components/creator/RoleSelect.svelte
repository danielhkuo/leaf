<script lang="ts">
  // The role a series is limited to. Nothing is chosen to begin with (a
  // default would gate the series on whichever role Discord listed first),
  // the roles arrive in Discord's own order with the creator's marked, and a
  // filter appears once the list is too long for a phone's picker.
  import type { RoleOption } from '../../types/api';

  interface Props {
    id: string;
    roles: RoleOption[];
    /** The chosen role id; `''` for none yet. */
    value: string;
    invalid?: boolean;
    describedby?: string | undefined;
    onchange?: () => void;
  }
  let { id, roles, value = $bindable(), invalid = false, describedby, onchange }: Props = $props();

  /** Past this many roles the native picker is a long list with no search. */
  const FILTER_FROM = 15;

  let query = $state('');
  const needle = $derived(query.trim().toLowerCase());
  const shown = $derived(
    needle === ''
      ? roles
      : roles.filter((r) => r.id === value || r.name.toLowerCase().includes(needle)),
  );
  const chosen = $derived(roles.find((r) => r.id === value) ?? null);
  /** A saved role Discord no longer lists: deleted, or the list is stale. */
  const gone = $derived(value !== '' && chosen === null);
  const matches = $derived.by(() => {
    if (needle === '') return '';
    const count = roles.filter((r) => r.name.toLowerCase().includes(needle)).length;
    if (count === 0) return 'No role matches. Check the spelling.';
    return `${count} of ${roles.length} roles match. Choose one below.`;
  });

  /** Return in the filter box must not submit the form around it. */
  function keepInForm(e: KeyboardEvent): void {
    if (e.key === 'Enter') e.preventDefault();
  }
</script>

<div class="role">
  {#if roles.length > FILTER_FROM}
    <input
      class="control"
      type="search"
      bind:value={query}
      placeholder="Filter roles"
      aria-label="Filter roles"
      autocomplete="off"
      autocapitalize="off"
      spellcheck="false"
      enterkeyhint="done"
      onkeydown={keepInForm}
    />
    <p class="hint" role="status">{matches}</p>
  {/if}
  <select
    {id}
    class="control"
    bind:value
    required
    aria-invalid={invalid ? 'true' : undefined}
    aria-describedby={describedby}
    {onchange}
  >
    <option value="" disabled>Choose a role…</option>
    {#if gone}<option {value}>A role that is no longer in this server</option>{/if}
    {#each shown as role (role.id)}
      <option value={role.id}>@{role.name}{role.held ? ' (you have it)' : ''}</option>
    {/each}
  </select>
  {#if chosen?.held === false}
    <p class="hint">You don’t have this role yourself. You’ll still see your own series.</p>
  {/if}
</div>

<style>
  .role {
    display: grid;
    gap: var(--space-xs);
  }
  .hint {
    margin: 0;
    color: var(--ink-muted);
    font-size: var(--fs-caption);
  }
  /* The match count stays in the page so a screen reader hears it change;
   * while empty it gives back the gap above it. */
  .hint:empty {
    margin-top: calc(-1 * var(--space-xs));
  }
  select[aria-invalid='true'] {
    border-color: var(--error-fill);
  }
</style>
