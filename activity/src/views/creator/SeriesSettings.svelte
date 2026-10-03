<script lang="ts">
  // Owner-only settings for one series. Loads the saved settings and the
  // server's options (channels, roles), saves the changed fields via PATCH,
  // and refreshes the gallery's list so the picker and home show a new name
  // or emoji. Leaving with unsaved changes asks first.
  //
  // The settings response does not have to carry the first day number, and
  // the form only offers a field whose current value it knows. The gallery's
  // list always has it, so it is filled in from there.
  import { onMount, tick } from 'svelte';

  import { unappliedFailure } from '../../lib/components/creator/settingsForm';
  import SeriesSettingsForm from '../../lib/components/creator/SeriesSettingsForm.svelte';
  import ErrorState from '../../lib/components/shared/ErrorState.svelte';
  import Skeleton from '../../lib/components/shared/Skeleton.svelte';
  import Button from '../../lib/components/ui/Button.svelte';
  import IconButton from '../../lib/components/ui/IconButton.svelte';
  import { gallery, getApi, getGuildId, refreshSeries } from '../../lib/stores/gallery.svelte';
  import { focusHeading, nav } from '../../lib/stores/nav.svelte';
  import type { SeriesOptions, SeriesSettings, UpdateSeriesInput } from '../../lib/types/api';
  import { ApiError, describeError, errorKind, isRetryable } from '../../lib/utils/errors';

  interface Props {
    seriesId: number;
  }
  let { seriesId }: Props = $props();

  const uid = $props.id();

  let settings = $state<SeriesSettings | null>(null);
  let options = $state<SeriesOptions | null>(null);
  let loadError = $state<unknown>(null);
  let attempt = $state(0);
  let saving = $state(false);
  let saved = $state(false);
  let failure = $state<{ code: string | null; message: string } | null>(null);
  /** Whether the form holds unsaved changes, and whether Back is asking about them. */
  let dirty = $state(false);
  let asking = $state(false);

  /** The series' first day number as the gallery's list has it. */
  function listedStartDay(id: number): number | undefined {
    return gallery.series.find((s) => s.id === id)?.start_day;
  }

  /** `saved` with its first day number filled in, when it has none and one is known. */
  function withStartDay(saved: SeriesSettings, known: number | undefined): SeriesSettings {
    return saved.start_day === undefined && known !== undefined
      ? { ...saved, start_day: known }
      : saved;
  }

  $effect(() => {
    void attempt;
    const api = getApi();
    const gid = getGuildId();
    const id = seriesId;
    let cancelled = false;
    settings = null;
    options = null;
    loadError = null;
    saved = false;
    failure = null;
    dirty = false;
    asking = false;
    Promise.all([api.getSettings(gid, id), api.getOptions(gid)])
      .then(([s, o]) => {
        if (cancelled) return;
        settings = withStartDay(s, listedStartDay(id));
        options = o;
      })
      .catch((e: unknown) => {
        console.error('leaf: loading the series settings failed', e);
        if (!cancelled) loadError = e;
      });
    return () => {
      cancelled = true;
    };
  });

  /** Fetches the options again in place (the role list did not load), keeping the form. */
  async function reloadOptions(): Promise<void> {
    try {
      options = await getApi().getOptions(getGuildId());
    } catch {
      // Still unavailable: the form keeps saying so.
    }
  }

  async function save(patch: UpdateSeriesInput): Promise<boolean> {
    const before = settings;
    if (before === null) return false;
    saving = true;
    saved = false;
    failure = null;
    try {
      const stored = await getApi().patchSeries(getGuildId(), seriesId, patch);
      // The picker and home show the name, emoji and privacy.
      const refreshed = refreshSeries();
      let startDay = before.start_day;
      if (stored.start_day === undefined && patch.start_day !== undefined) {
        // The answer does not say whether the first day number moved. The
        // refreshed list does; without it, the server's 200 is taken at its word.
        startDay = ((await refreshed) ? listedStartDay(seriesId) : undefined) ?? patch.start_day;
      }
      const after = withStartDay(stored, startDay);
      settings = after;
      // A save that left the name or first day number as it was did not go through.
      failure = unappliedFailure(before, patch, after);
      saved = failure === null;
      return saved;
    } catch (e) {
      console.error('leaf: saving the series settings failed', e);
      const code = e instanceof ApiError ? (e.code ?? null) : null;
      if (code === 'revoked') {
        // Revoked while the form was open: load it again, read-only.
        attempt += 1;
      } else {
        failure = { code, message: describeError(e) };
      }
      return false;
    } finally {
      saving = false;
    }
  }

  // Whether any day is archived, for the reminder summary. Unknown when the
  // series is not in the gallery's list.
  const hasDays = $derived.by(() => {
    const listed = gallery.series.find((s) => s.id === seriesId);
    return listed ? listed.max_day !== null : undefined;
  });

  // --- leaving with unsaved changes -------------------------------------

  let prompt = $state<HTMLElement>();

  /**
   * Back with unsaved changes asks first. A second press does not count as
   * the answer (it is too easy to tap twice): only "Discard changes" leaves.
   */
  async function back(): Promise<void> {
    if (!dirty) {
      nav.back();
      return;
    }
    asking = true;
    await tick();
    prompt?.querySelector('button')?.focus();
  }

  function dirtyChanged(now: boolean): void {
    dirty = now;
    // Saved (or edited back) while the question was up: it no longer applies.
    if (!now) asking = false;
  }

  function keepEditing(): void {
    asking = false;
    void tick().then(() => focusHeading(root));
  }

  let root = $state<HTMLElement>();
  onMount(() => focusHeading(root));
</script>

<div class="view" bind:this={root}>
  <header class="bar">
    <IconButton ariaLabel="Back" variant="solid" icon="back" onclick={() => void back()} />
    <h1 tabindex="-1">Series settings</h1>
  </header>

  {#if asking}
    <div class="discard" role="group" aria-labelledby="{uid}-discard" bind:this={prompt}>
      <p class="discard-title" id="{uid}-discard">Leave without saving?</p>
      <p class="discard-text">The changes you made on this screen will be lost.</p>
      <div class="discard-actions">
        <Button variant="secondary" onclick={keepEditing}>Keep editing</Button>
        <Button variant="danger" onclick={() => nav.back()}>Discard changes</Button>
      </div>
    </div>
  {/if}

  {#if settings && options}
    <SeriesSettingsForm
      {settings}
      {options}
      {saving}
      {saved}
      error={failure?.message ?? null}
      errorCode={failure?.code}
      {hasDays}
      onSave={save}
      onDirtyChange={dirtyChanged}
      onReloadOptions={() => void reloadOptions()}
    />
  {:else if loadError}
    {#if errorKind(loadError) === 'not_found'}
      <ErrorState
        title="These settings aren’t available"
        message="Only the member who started a series can change its settings. If this is your series, it may have been removed."
      />
    {:else}
      <ErrorState
        title="Couldn’t load the settings"
        message={describeError(loadError)}
        onRetry={isRetryable(loadError) ? () => (attempt += 1) : undefined}
      />
    {/if}
  {:else}
    <Skeleton height="360px" radius="var(--radius-xl)" label="Loading the settings" />
  {/if}
</div>

<style>
  .view {
    /* The form's sticky save bar reaches the screen edges through this. */
    --form-bleed: var(--space-md);
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-md);
    width: 100%;
    max-width: 44rem;
    margin: 0 auto;
    padding: var(--space-md);
  }
  .bar {
    display: flex;
    gap: var(--space-sm);
    align-items: center;
    min-height: var(--appbar-h);
  }
  h1 {
    margin: 0;
    font-size: var(--fs-card-title);
    font-weight: var(--fw-display);
    letter-spacing: var(--tracking-display);
  }
  /* Focused by script when the screen appears; a ring there means nothing. */
  h1:focus {
    outline: none;
  }
  .discard {
    padding: var(--space-md);
    background: var(--surface-1);
    border: 1px solid var(--hairline);
    border-left: 2px solid var(--warning-fill);
    border-radius: var(--radius-xl);
  }
  .discard p {
    margin: 0;
  }
  .discard-title {
    font-weight: var(--fw-emphasis);
  }
  .discard-text {
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
  }
  .discard-actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-xs);
    margin-top: var(--space-md);
  }
</style>
