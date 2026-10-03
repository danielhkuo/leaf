<script lang="ts">
  // The "start a series" view: load eligibility and options, then either say
  // what is in the way or show the form. A successful create puts the new
  // series into the gallery, remembers it as the one to open next time, and
  // lands on its home, which explains how to archive the first post.
  import { onMount } from 'svelte';

  import CreateWizard from '../../lib/components/creator/CreateWizard.svelte';
  import ViolationCallout from '../../lib/components/creator/ViolationCallout.svelte';
  import ErrorState from '../../lib/components/shared/ErrorState.svelte';
  import Skeleton from '../../lib/components/shared/Skeleton.svelte';
  import IconButton from '../../lib/components/ui/IconButton.svelte';
  import { resetDraft } from '../../lib/stores/createDraft.svelte';
  import {
    adoptSeries,
    getApi,
    getGuildId,
    refreshEligibility,
    rememberSeries,
  } from '../../lib/stores/gallery.svelte';
  import { focusHeading, nav } from '../../lib/stores/nav.svelte';
  import { session } from '../../lib/stores/session.svelte';
  import type { CreateSeriesInput, SeriesOptions, Violation } from '../../lib/types/api';
  import { ApiError, describeError, isRetryable } from '../../lib/utils/errors';

  type Load =
    | { status: 'loading' }
    | { status: 'failed'; error: unknown }
    | { status: 'blocked'; violations: Violation[] }
    | { status: 'ready'; options: SeriesOptions };

  let load = $state<Load>({ status: 'loading' });
  let attempt = $state(0);
  let submitting = $state(false);
  let failure = $state<{ code: string | null; message: string } | null>(null);

  /** The channel leaf was opened in: the form preselects it when it can. */
  const launchChannelId = $derived(
    session.value.status === 'authed' ? session.value.session.channelId : null,
  );

  type Settled<T> = { ok: true; value: T } | { ok: false; error: unknown };

  function settle<T>(request: Promise<T>): Promise<Settled<T>> {
    return request.then(
      (value) => ({ ok: true, value }),
      (error: unknown) => ({ ok: false, error }),
    );
  }

  function codeOf(e: unknown): string | null {
    return e instanceof ApiError ? (e.code ?? null) : null;
  }

  $effect(() => {
    void attempt;
    const api = getApi();
    const gid = getGuildId();
    let cancelled = false;
    load = { status: 'loading' };
    void Promise.all([settle(api.getEligibility(gid)), settle(api.getOptions(gid))]).then(
      ([eligibility, options]) => {
        if (cancelled) return;
        if (eligibility.ok && !eligibility.value.can_create) {
          load = { status: 'blocked', violations: eligibility.value.violations };
        } else if (options.ok) {
          // A failed eligibility check alone does not block the form: the
          // server checks again when the series is submitted.
          load = { status: 'ready', options: options.value };
        } else if (codeOf(options.error) === 'guild_not_setup') {
          load = { status: 'blocked', violations: [{ code: 'guild_not_setup', message: '' }] };
        } else {
          console.error('leaf: loading the create form failed', options.error);
          load = { status: 'failed', error: options.error };
        }
      },
    );
    return () => {
      cancelled = true;
    };
  });

  /** Fetches the options again in place (the role list did not load), keeping the form. */
  async function reloadOptions(): Promise<void> {
    const fresh = await settle(getApi().getOptions(getGuildId()));
    if (fresh.ok && load.status === 'ready') load = { status: 'ready', options: fresh.value };
  }

  async function submit(input: CreateSeriesInput): Promise<void> {
    submitting = true;
    failure = null;
    try {
      // A current server answers a repeat of the same create (after a lost
      // response) with the series it already made, so trying again is safe.
      const created = await getApi().createSeries(getGuildId(), input);
      await adoptSeries(created);
      rememberSeries(created.id);
      // The series may have been the last one this member is allowed.
      void refreshEligibility();
      resetDraft();
      nav.reset({ name: 'picker' });
      nav.push({ name: 'home', seriesId: created.id, created: true });
    } catch (e) {
      console.error('leaf: creating the series failed', e);
      failure = { code: codeOf(e), message: describeError(e) };
    } finally {
      submitting = false;
    }
  }

  let root = $state<HTMLElement>();
  onMount(() => focusHeading(root));
</script>

<div class="view" bind:this={root}>
  <header class="bar">
    <IconButton ariaLabel="Back" variant="solid" icon="back" onclick={() => nav.back()} />
    <h1 tabindex="-1">Start a series</h1>
  </header>

  {#if load.status === 'loading'}
    <Skeleton height="320px" radius="var(--radius-xl)" label="Loading the form" />
  {:else if load.status === 'failed'}
    {@const error = load.error}
    <ErrorState
      title="Couldn’t load this screen"
      message={describeError(error)}
      onRetry={isRetryable(error) ? () => (attempt += 1) : undefined}
    />
  {:else if load.status === 'blocked'}
    <ViolationCallout violations={load.violations} />
  {:else}
    <CreateWizard
      options={load.options}
      {submitting}
      error={failure?.message ?? null}
      errorCode={failure?.code}
      channelId={launchChannelId}
      onSubmit={(input) => void submit(input)}
      onReloadOptions={() => void reloadOptions()}
    />
  {/if}
</div>

<style>
  .view {
    /* The form's sticky action bar reaches the screen edges through this. */
    --form-bleed: var(--space-md);
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-md);
    width: 100%;
    max-width: 40rem;
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
</style>
