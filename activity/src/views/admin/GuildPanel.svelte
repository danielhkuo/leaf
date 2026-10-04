<script lang="ts">
  // One server's panel: its name, its settings form and its series. It loads
  // the server and the pickers' lists, keeps both current as things are
  // saved, and is the one place that notices an expired sign-in: unsaved
  // settings are stashed first, so they are back after signing in again.
  import { untrack } from 'svelte';

  import { isUnauthorized, type AdminApi } from '../../lib/admin/client';
  import { adminErrorMessage, canRetry, isGone } from '../../lib/admin/copy';
  import type {
    AdminGuildDetail,
    AdminOptions,
    AdminSeries,
    AdminSettings,
  } from '../../lib/admin/schemas';
  import { stashDraft, takeDraft } from '../../lib/admin/session';
  import { readDraft, type SettingsDraft } from '../../lib/admin/settingsForm';
  import Callout from '../../lib/components/shared/Callout.svelte';
  import ErrorState from '../../lib/components/shared/ErrorState.svelte';
  import Skeleton from '../../lib/components/shared/Skeleton.svelte';
  import GuildIcon from './GuildIcon.svelte';
  import type { Choice } from './IdPicker.svelte';
  import SeriesRow from './SeriesRow.svelte';
  import SettingsForm from './SettingsForm.svelte';

  interface Props {
    api: AdminApi;
    guildId: string;
    /** The server's name and icon from the server list, shown until its own details load. */
    name?: string | undefined;
    iconUrl?: string | undefined;
    /** The admin token was refused. `draftKept` says unsaved settings were stashed. */
    onUnauthorized?: ((draftKept: boolean) => void) | undefined;
    /** Told whenever the settings form starts or stops having unsaved changes. */
    onDirtyChange?: ((dirty: boolean) => void) | undefined;
    /** Leaves a server that did not load, where there is a list to go back to. */
    onBack?: (() => void) | undefined;
    /** Starts a fresh sign-in, for a server this one can no longer manage. */
    onSignIn?: (() => void) | undefined;
  }
  let { api, guildId, name, iconUrl, onUnauthorized, onDirtyChange, onBack, onSignIn }: Props =
    $props();

  interface Failure {
    message: string;
    /** Trying again can help. */
    retry: boolean;
    /** This sign-in can't manage the server any more. */
    gone: boolean;
  }

  const uid = $props.id();

  let detail = $state<AdminGuildDetail | null>(null);
  let failure = $state<Failure | null>(null);
  /** `undefined` while the pickers' lists load; `null` when leaf could not get them. */
  let options = $state<AdminOptions | null | undefined>(undefined);
  let restored = $state<SettingsDraft | null>(null);
  /** Bumped to load the same server again. */
  let attempt = $state(0);

  // The settings form's unsaved fields, kept current so they can be stashed
  // the moment a request comes back 401.
  let draft: SettingsDraft | null = null;
  /** Counts loads, so an answer for a server no longer on screen is dropped. */
  let epoch = 0;
  let signedOut = false;

  const title = $derived(detail?.name ?? name ?? `Server ${guildId}`);
  const roles = $derived.by((): Choice[] | null | undefined => {
    if (options === undefined) return undefined;
    if (options === null || options.roles === undefined || options.roles_unavailable) return null;
    return options.roles.map((role) => ({ id: role.id, label: `@${role.name}` }));
  });

  function expired(): void {
    if (signedOut) return;
    signedOut = true;
    // Only what the browser really took is promised back on the sign-in card.
    const kept = draft !== null && stashDraft(guildId, draft);
    if (onUnauthorized) {
      onUnauthorized(kept);
      return;
    }
    detail = null;
    failure = {
      message: 'Your admin session has expired. Sign in again.',
      retry: false,
      gone: false,
    };
  }

  async function loadOptions(gid: string, mine: number): Promise<void> {
    options = undefined;
    try {
      const loaded = await api.options(gid);
      if (mine === epoch) options = loaded;
    } catch (e) {
      if (mine !== epoch) return;
      if (isUnauthorized(e)) expired();
      // An older server has no such route: the form falls back to text boxes.
      else options = null;
    }
  }

  async function load(gid: string, mine: number): Promise<void> {
    detail = null;
    failure = null;
    setDraft(null);
    void loadOptions(gid, mine);
    try {
      const loaded = await api.guild(gid);
      if (mine !== epoch) return;
      restored = readDraft(takeDraft(gid));
      detail = loaded;
    } catch (e) {
      if (mine !== epoch) return;
      if (isUnauthorized(e)) expired();
      else failure = { message: adminErrorMessage(e), retry: canRetry(e), gone: isGone(e) };
    }
  }

  // Load the server whenever it changes, or Try again is pressed.
  $effect(() => {
    const gid = guildId;
    void attempt;
    epoch += 1;
    const mine = epoch;
    untrack(() => void load(gid, mine));
    return () => {
      epoch += 1;
    };
  });

  function setDraft(next: SettingsDraft | null): void {
    draft = next;
    onDirtyChange?.(next !== null);
  }

  /**
   * Loads the rows again: after a save that published sprouts, and for a row
   * whose change leaf never confirmed. Resolves to the fresh rows, or `null`
   * when they could not be loaded and the list stays as it was.
   */
  async function refreshSeries(): Promise<AdminSeries[] | null> {
    const mine = epoch;
    try {
      const fresh = await api.guild(guildId);
      if (mine !== epoch || !detail) return null;
      detail = { ...detail, series: fresh.series };
      return fresh.series;
    } catch (e) {
      if (mine === epoch && isUnauthorized(e)) expired();
      return null;
    }
  }

  /** One series as leaf stores it now; `null` when it could not be read or is gone. */
  async function rereadSeries(id: number): Promise<AdminSeries | null> {
    const fresh = await refreshSeries();
    return fresh?.find((series) => series.id === id) ?? null;
  }

  function settingsSaved(updated: AdminSettings): void {
    if (!detail) return;
    detail = { ...detail, settings: updated };
    if ((updated.sprouts_published ?? 0) > 0) void refreshSeries();
  }

  function seriesChanged(updated: AdminSeries): void {
    if (!detail) return;
    detail = {
      ...detail,
      series: detail.series.map((old) =>
        old.id === updated.id
          ? {
              ...updated,
              // A PATCH answer may leave out what the list carried.
              creator_name: updated.creator_name ?? old.creator_name,
              archived_days: updated.archived_days ?? old.archived_days,
              privacy_role_name:
                updated.privacy_role_name ??
                (updated.privacy_role_id === old.privacy_role_id
                  ? old.privacy_role_name
                  : undefined),
            }
          : old,
      ),
    };
  }
</script>

<div class="guild">
  <div class="heading">
    <GuildIcon name={title} url={detail?.icon_url ?? iconUrl} size={48} />
    <h1 tabindex="-1">{title}</h1>
  </div>

  {#if failure}
    <ErrorState
      title={failure.gone ? 'This server isn’t available' : 'Couldn’t load this server'}
      message={failure.message}
      onRetry={failure.gone ? onSignIn : failure.retry ? () => (attempt += 1) : undefined}
      retryLabel={failure.gone ? 'Sign in again' : 'Try again'}
      {onBack}
      backLabel="Choose another server"
    />
  {:else if !detail}
    <Skeleton height="18rem" radius="var(--radius-xl)" label="Loading this server" />
  {:else}
    {#if detail.setup_complete === false}
      <Callout title="leaf isn’t set up in this server yet" tone="warning">
        Run /setup in the server to choose the channels people can archive from. Until then nobody
        can start a series. The settings below still save.
      </Callout>
    {/if}

    <section class="part" aria-labelledby="{uid}-settings">
      <h2 id="{uid}-settings">Settings</h2>
      <SettingsForm
        settings={detail.settings}
        {options}
        {restored}
        save={(patch) => api.patchSettings(guildId, patch)}
        onSaved={settingsSaved}
        onDraft={setDraft}
        onUnauthorized={expired}
        onReloadOptions={() => void loadOptions(guildId, epoch)}
      />
    </section>

    <section class="part" aria-labelledby="{uid}-series">
      <h2 id="{uid}-series">Series</h2>
      <p class="lead">
        Every series in this server, whatever its privacy. In the gallery itself, admins see what
        any other member sees. Revoking hides a series from the gallery and stops new posts; its
        archived days are kept, and you can restore it.
      </p>
      {#if detail.series.length === 0}
        <p class="empty">No series yet. Members start one from leaf’s gallery in Discord.</p>
      {:else}
        <ul class="series">
          {#each detail.series as series (series.id)}
            <SeriesRow
              {series}
              {roles}
              threshold={detail.settings.sprout_threshold}
              save={(patch) => api.patchSeries(guildId, series.id, patch)}
              onChanged={seriesChanged}
              reread={() => rereadSeries(series.id)}
              onUnauthorized={expired}
            />
          {/each}
        </ul>
      {/if}
    </section>
  {/if}
</div>

<style>
  .guild {
    /* The page pads its sides by this much; the settings form's sticky Save
     * bar uses it to reach the screen edges. */
    --form-bleed: var(--space-md);

    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-lg);
    margin-top: var(--space-lg);
  }
  .heading {
    display: flex;
    gap: var(--space-sm);
    align-items: center;
    min-width: 0;
  }
  h1 {
    min-width: 0;
    margin: 0;
    font-size: var(--fs-card-title);
    font-weight: var(--fw-display);
    letter-spacing: var(--tracking-display);
    line-height: 1.2;
    overflow-wrap: anywhere;
  }
  /* Focused by script when the panel opens; it is not a control. */
  h1:focus {
    outline: none;
  }
  .part {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-sm);
  }
  h2 {
    margin: 0;
    font-size: var(--fs-subhead);
    font-weight: var(--fw-display);
  }
  .lead,
  .empty {
    margin: 0;
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
  }
  .lead {
    max-width: 44rem;
  }
  .series {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-sm);
    margin: 0;
    padding: 0;
    list-style: none;
  }
</style>
