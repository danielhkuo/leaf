<script lang="ts">
  // One series in the admin's list: who made it, its state, who can see it,
  // and the actions an admin has (revoke, restore, publish a sprout).
  //
  // Nothing here applies on a bare tap. A privacy change waits for Save (a
  // slip in a phone's picker must not publish a private series), and Revoke
  // asks first. While a request is out the row is disabled; when it fails the
  // controls go back to what is stored and the row says why, right here.
  //
  // The row only says a change did or didn't happen when leaf's copy of the
  // series says so. A request that got no answer may still have been applied,
  // so the row reads the series again before it says anything, and holds its
  // controls for as long as it cannot.
  import { tick } from 'svelte';

  import { isUnauthorized } from '../../lib/admin/client';
  import {
    adminErrorMessage,
    adminPrivacyLabel,
    creatorLabel,
    idTail,
    outcomeUnknown,
    PRIVACY_MODES,
    privacyQuestion,
    stateLabel,
    unconfirmedMessage,
  } from '../../lib/admin/copy';
  import type { AdminSeries, SeriesPatch } from '../../lib/admin/schemas';
  import { isSnowflake } from '../../lib/admin/settingsForm';
  import Button from '../../lib/components/ui/Button.svelte';
  import IdPicker, { type Choice } from './IdPicker.svelte';

  interface Props {
    series: AdminSeries;
    /** Roles a series can be limited to: `undefined` while loading, `null` when unavailable. */
    roles: Choice[] | null | undefined;
    /** The server's sprout threshold, for a sprout's progress. */
    threshold: number;
    /** Sends a change; resolves to the series as the server stored it. */
    save: (patch: SeriesPatch) => Promise<AdminSeries>;
    /** A change went through. The parent must hand the result back as `series`. */
    onChanged: (updated: AdminSeries) => void;
    /**
     * Reads the series as leaf stores it now, for a change nobody confirmed.
     * The parent hands the result back as `series` too; `null` when it could
     * not be read. Must not reject.
     */
    reread: () => Promise<AdminSeries | null>;
    /** The server refused the admin token. */
    onUnauthorized: () => void;
  }
  let { series, roles, threshold, save, onChanged, reread, onUnauthorized }: Props = $props();

  type Action = 'privacy' | 'revoke' | 'restore' | 'publish';

  /** One change sent to leaf, and what the row says about how it went. */
  interface Change {
    action: Action;
    patch: SeriesPatch;
    /** "… wasn't revoked." Said when leaf refuses, or when its copy of the series lacks the change. */
    wasnt: string;
    /** The confirmation, or `null` when the series as leaf stores it lacks the change. */
    done: (stored: AdminSeries) => string | null;
  }

  /** A change leaf neither confirmed nor refused, and the failure that left it open. */
  interface Unsure {
    change: Change;
    cause: unknown;
  }

  const uid = $props.id();

  // Unsaved picks. `null` means "what is stored", so a failed or cancelled
  // change puts the controls back by itself.
  let draftPrivacy = $state<string | null>(null);
  let draftRole = $state<string | null>(null);
  let asking = $state(false);
  let pending = $state<Action | 'check' | null>(null);
  /** Set while nobody knows whether the last change is stored. */
  let unsure = $state.raw<Unsure | null>(null);
  let error = $state('');
  let note = $state('');

  let row = $state<HTMLElement>();
  let acts = $state<HTMLElement>();
  let ask = $state<HTMLElement>();
  let recheck = $state<HTMLElement>();

  const busy = $derived(pending !== null);
  /** No control may act on a state the row is not sure of. */
  const held = $derived(busy || unsure !== null);
  const storedRole = $derived(series.privacy_role_id ?? '');
  const privacy = $derived(draftPrivacy ?? series.privacy);
  const roleId = $derived(draftRole ?? storedRole);
  const gated = $derived(privacy === 'role_gated');
  const changed = $derived(privacy !== series.privacy || (gated && roleId !== storedRole));
  /** A picked role is always usable; a pasted one has to look like an id. */
  const roleReady = $derived(roles === null ? isSnowflake(roleId.trim()) : roleId !== '');

  function roleName(id: string): string | null {
    if (id === '') return null;
    const listed = roles?.find((role) => role.id === id);
    if (listed) return listed.label;
    if (id === storedRole && series.privacy_role_name) return `@${series.privacy_role_name}`;
    return roles === null ? `the role with ID ${id}` : `a role leaf can’t see (${idTail(id)})`;
  }

  function unlistedRole(id: string): string {
    return id === storedRole && series.privacy_role_name
      ? `@${series.privacy_role_name}`
      : `A role leaf can’t see (${idTail(id)})`;
  }

  function quiet(): void {
    error = '';
    note = '';
  }

  function choosePrivacy(picked: string): void {
    quiet();
    draftPrivacy = picked === series.privacy ? null : picked;
    if (picked !== 'role_gated') draftRole = null;
  }

  function chooseRole(picked: string): void {
    quiet();
    draftRole = picked === storedRole ? null : picked;
  }

  /** Puts focus back in the row once the control that had it is gone or disabled. */
  async function refocus(target: () => HTMLElement | null | undefined): Promise<void> {
    await tick();
    const active = document.activeElement;
    // Someone who has moved on to another control keeps their place.
    if (active && active !== document.body && !row?.contains(active)) return;
    target()?.focus();
  }

  const lastAction = (): HTMLElement | null =>
    [...(acts?.querySelectorAll<HTMLElement>('button') ?? [])].pop() ?? null;
  const privacySelect = (): HTMLElement | null => document.getElementById(`${uid}-privacy`);
  const recheckButton = (): HTMLElement | null => recheck?.querySelector('button') ?? null;

  /** Where focus belongs once a change has been dealt with. */
  function home(action: Action): HTMLElement | null {
    if (unsure !== null) return recheckButton();
    return action === 'privacy' ? privacySelect() : lastAction();
  }

  /**
   * Says how a change went, going by the series as leaf stores it. `why` is
   * the sentence after "wasn't …" when the change is not there.
   */
  function report(change: Change, stored: AdminSeries, why: string): void {
    const said = change.done(stored);
    if (said === null) error = `${change.wasnt} ${why}`;
    else note = said;
  }

  /**
   * A change got no answer, so leaf may have stored it. Reads the series
   * again and says what is there. If that fails too, the row is held with
   * "Check again", because its controls would act on a state nobody knows.
   */
  async function check(open: Unsure): Promise<void> {
    note = 'Checking what leaf has stored…';
    let stored: AdminSeries | null = null;
    try {
      stored = await reread();
    } catch {
      // Counts as not read.
    }
    note = '';
    if (stored === null) {
      unsure = open;
      error = unconfirmedMessage(open.cause);
      return;
    }
    unsure = null;
    report(open.change, stored, adminErrorMessage(open.cause, 'series'));
  }

  async function run(change: Change): Promise<void> {
    quiet();
    pending = change.action;
    try {
      const updated = await save(change.patch);
      onChanged(updated);
      // leaf can answer with the series unchanged: a sprout it won't publish.
      const sprout = change.action === 'publish' && updated.state === 'sprout';
      report(change, updated, sprout ? 'leaf kept it as a sprout.' : 'leaf kept it as it was.');
    } catch (e) {
      if (isUnauthorized(e)) {
        onUnauthorized();
        return;
      }
      if (outcomeUnknown(e)) await check({ change, cause: e });
      else error = `${change.wasnt} ${adminErrorMessage(e, 'series')}`;
    } finally {
      // Saved or not, the privacy controls show what is stored again. A pick
      // still waiting for Save outlives a revoke or restore next to it.
      if (change.action === 'privacy') {
        draftPrivacy = null;
        draftRole = null;
      }
      asking = false;
      pending = null;
    }
    void refocus(() => home(change.action));
  }

  async function checkAgain(): Promise<void> {
    const open = unsure;
    if (open === null || busy) return;
    error = '';
    pending = 'check';
    try {
      await check(open);
    } finally {
      pending = null;
    }
    void refocus(() => home(open.change.action));
  }

  function savePrivacy(): void {
    if (held || !changed || (gated && !roleReady)) return;
    // Role-only always goes with its role, so the two can't come apart.
    const patch: SeriesPatch = gated ? { privacy, privacy_role_id: roleId.trim() } : { privacy };
    void run({
      action: 'privacy',
      patch,
      wasnt: `Who can see “${series.name}” wasn’t changed.`,
      done: (stored) =>
        stored.privacy === patch.privacy &&
        (patch.privacy_role_id === undefined || stored.privacy_role_id === patch.privacy_role_id)
          ? 'Saved'
          : null,
    });
  }

  function cancelPrivacy(): void {
    draftPrivacy = null;
    draftRole = null;
    void refocus(privacySelect);
  }

  async function askRevoke(): Promise<void> {
    quiet();
    asking = true;
    await tick();
    ask?.focus();
  }

  function cancelRevoke(): void {
    asking = false;
    void refocus(lastAction);
  }

  function revoke(): void {
    void run({
      action: 'revoke',
      patch: { state: 'revoked' },
      wasnt: `“${series.name}” wasn’t revoked.`,
      done: (stored) =>
        stored.state === 'revoked'
          ? 'Revoked. It’s hidden from the gallery and can’t take new posts.'
          : null,
    });
  }

  function restore(): void {
    void run({
      action: 'restore',
      patch: { state: 'active' },
      wasnt: `“${series.name}” wasn’t restored.`,
      // The server decides between active and sprout from the days archived.
      done: (stored) => {
        if (stored.state === 'revoked') return null;
        return stored.state === 'sprout'
          ? 'Restored as a sprout: it doesn’t have enough archived days to be published yet.'
          : 'Restored.';
      },
    });
  }

  function publish(): void {
    void run({
      action: 'publish',
      patch: { state: 'active' },
      wasnt: `“${series.name}” wasn’t published.`,
      // A server that holds every series to the sprout threshold answers
      // with the sprout it still is.
      done: (stored) =>
        stored.state === 'active'
          ? 'Published. It now shows in the gallery to whoever its privacy allows.'
          : null,
    });
  }
</script>

<li class="row" class:revoked={series.state === 'revoked'} bind:this={row} aria-busy={busy}>
  <div class="top">
    <div class="meta">
      <span class="name">{series.name}</span>
      <span class="sub">{creatorLabel(series)} · {stateLabel(series, threshold)}</span>
    </div>

    <div class="privacy">
      <div class="field">
        <label class="field-label" for="{uid}-privacy">
          Who can see it<span class="sr-only">: {series.name}</span>
        </label>
        <select
          id="{uid}-privacy"
          class="control"
          value={privacy}
          disabled={held}
          onchange={(e) => choosePrivacy(e.currentTarget.value)}
        >
          {#each PRIVACY_MODES as mode (mode)}
            <option value={mode}>{adminPrivacyLabel(mode)}</option>
          {/each}
          <!-- A stored value this build does not know still has to show. -->
          {#if !PRIVACY_MODES.includes(series.privacy)}
            <option value={series.privacy}>{series.privacy}</option>
          {/if}
        </select>
      </div>

      {#if gated}
        <div class="field">
          <label class="field-label" for="{uid}-role">
            Role <span class="sr-only">that can see {series.name}</span>
          </label>
          <IdPicker
            id="{uid}-role"
            choices={roles}
            bind:value={() => roleId, chooseRole}
            stored={storedRole}
            placeholder="Choose a role…"
            unlisted={unlistedRole}
            idHint="Role ID"
            disabled={held}
            describedby={roles === null ? `${uid}-role-hint` : undefined}
          />
          {#if roles === null}
            <p class="hint" id="{uid}-role-hint">
              leaf couldn’t load this server’s roles. Paste the role’s ID, a number 17 to 20 digits
              long.
            </p>
          {/if}
        </div>
      {/if}

      {#if changed}
        <div class="unsaved" role="group" aria-labelledby="{uid}-change">
          <p id="{uid}-change">{privacyQuestion(series.name, privacy, roleName(roleId.trim()))}</p>
          <div class="buttons">
            <Button
              variant="primary"
              size="sm"
              disabled={held || (gated && !roleReady)}
              onclick={savePrivacy}
            >
              {pending === 'privacy' ? 'Saving…' : 'Save'}
            </Button>
            <Button variant="ghost" size="sm" disabled={held} onclick={cancelPrivacy}>Cancel</Button
            >
          </div>
        </div>
      {:else if gated && storedRole === ''}
        <p class="hint caution">
          No role is set, so only its creator can see this series. Choose a role, or change who can
          see it.
        </p>
      {/if}
    </div>

    <div class="acts" bind:this={acts}>
      <!-- The question below replaces the buttons, so a second tap can't land on Revoke. -->
      {#if !asking}
        {#if series.state === 'revoked'}
          <Button size="sm" disabled={held} onclick={restore}>
            {pending === 'restore' ? 'Restoring…' : 'Restore'}
          </Button>
        {:else}
          {#if series.state === 'sprout'}
            <Button size="sm" disabled={held} onclick={publish}>
              {pending === 'publish' ? 'Publishing…' : 'Publish now'}
            </Button>
          {/if}
          <Button variant="danger" size="sm" disabled={held} onclick={askRevoke}>Revoke</Button>
        {/if}
      {/if}
    </div>
  </div>

  {#if asking}
    <div class="ask" role="group" aria-labelledby="{uid}-ask" tabindex="-1" bind:this={ask}>
      <p id="{uid}-ask">
        Hide “{series.name}” from the gallery and stop new posts? Its archived days are kept, and
        you can restore it later.
      </p>
      <div class="buttons">
        <Button variant="danger" size="sm" disabled={busy} onclick={revoke}>
          {pending === 'revoke' ? 'Revoking…' : 'Revoke'}
        </Button>
        <Button variant="ghost" size="sm" disabled={busy} onclick={cancelRevoke}>Cancel</Button>
      </div>
    </div>
  {/if}

  {#if error}<p class="row-error" role="alert">{error}</p>{/if}
  {#if unsure}
    <div class="recheck" bind:this={recheck}>
      <Button size="sm" disabled={busy} onclick={checkAgain}>
        {pending === 'check' ? 'Checking…' : 'Check again'}
      </Button>
    </div>
  {/if}
  <p class="row-note" class:working={busy} role="status">{note}</p>
</li>

<style>
  .row {
    padding: var(--space-md);
    background: var(--surface-2);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-lg);
  }
  /* Phones: name, then who can see it, then the actions, each full width. */
  .top {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    grid-template-areas:
      'meta'
      'privacy'
      'acts';
    gap: var(--space-sm);
  }
  .meta {
    display: grid;
    grid-area: meta;
    gap: 2px;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .name {
    font-weight: var(--fw-emphasis);
  }
  .sub {
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
  }
  /* A revoked series is set back by its name alone: the Restore button and
   * the rest of the row stay at full strength. */
  .revoked .name {
    color: var(--ink-muted);
  }
  .privacy {
    display: grid;
    grid-area: privacy;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-sm);
    align-items: start;
  }
  /* Side by side, a field with a hint is taller: the other keeps its size. */
  .field {
    align-content: start;
    min-width: 0;
  }
  .acts {
    display: flex;
    flex-wrap: wrap;
    grid-area: acts;
    gap: var(--space-xs);
  }
  .acts:empty {
    display: none;
  }
  .hint,
  .unsaved p,
  .ask p,
  .row-error,
  .row-note {
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
  /* One shrinkable column: the question quotes the series name, which can
   * be 40 characters with no space to wrap at. */
  .unsaved,
  .ask {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-xs);
    padding: var(--space-sm);
    background: var(--surface-1);
    border: 1px solid var(--hairline-strong);
    border-radius: var(--radius-md);
  }
  .ask {
    margin-top: var(--space-sm);
    outline: none;
  }
  .buttons {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-xs);
  }
  .row-error,
  .recheck,
  .row-note:not(:empty) {
    margin-top: var(--space-sm);
  }
  .row-error {
    color: var(--error);
  }
  .row-note {
    color: var(--success);
    font-weight: var(--fw-emphasis);
  }
  /* "Checking…" is progress, not an outcome. */
  .row-note.working {
    color: var(--ink-muted);
    font-weight: inherit;
  }

  @media (min-width: 560px) {
    .top {
      grid-template-columns: minmax(0, 1fr) auto;
      grid-template-areas:
        'meta acts'
        'privacy privacy';
      align-items: start;
    }
    .privacy {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
    .unsaved,
    .caution {
      grid-column: 1 / -1;
    }
    .acts {
      justify-content: flex-end;
    }
  }
</style>
