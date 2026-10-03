<script lang="ts">
  // The admin page's shell: sign-in, the server list and one server's panel.
  // The open server lives in the URL (`/admin?guild=<id>`), so Back, refresh
  // and links all work, and it survives the round trip through Discord's
  // sign-in. Leaving unsaved settings (Switch server, Sign out, Back, closing
  // the tab) asks first.
  import { onMount, tick } from 'svelte';

  import { AdminApi, isUnauthorized } from '../../lib/admin/client';
  import {
    adminErrorMessage,
    canRetry,
    NO_GUILDS,
    sessionExpired,
    signInProblem,
    type SignInProblem,
  } from '../../lib/admin/copy';
  import type { AdminGuild } from '../../lib/admin/schemas';
  import {
    adminUrl,
    forgetTab,
    guildInUrl,
    parseFragment,
    readToken,
    rememberGuild,
    storeToken,
    takeRememberedGuild,
  } from '../../lib/admin/session';
  import Callout from '../../lib/components/shared/Callout.svelte';
  import ErrorState from '../../lib/components/shared/ErrorState.svelte';
  import Button from '../../lib/components/ui/Button.svelte';
  import Spinner from '../../lib/components/ui/Spinner.svelte';
  import GuildIcon from './GuildIcon.svelte';
  import GuildPanel from './GuildPanel.svelte';

  type View =
    | { status: 'loading' }
    | { status: 'login'; problem: SignInProblem | null }
    | { status: 'guilds'; note: string | null }
    | { status: 'panel'; guildId: string }
    | { status: 'error'; message: string; retry: boolean };

  let view = $state<View>({ status: 'loading' });
  // Reassigned on login/logout; the panel branch reads it, so it must be state.
  let api = $state<AdminApi | null>(null);
  let guilds = $state<AdminGuild[]>([]);
  let panelDirty = $state(false);
  /** What the admin asked to do while settings were unsaved; set while the page asks. */
  let leaving = $state<(() => void) | null>(null);

  let root = $state<HTMLElement>();
  let guard = $state<HTMLElement>();
  /** What had focus when the page started asking, to hand it back on "Keep editing". */
  let asker: HTMLElement | null = null;

  // The server to open once the list is in: named by the URL, or the one a
  // session ran out on. Carried through sign-in in sessionStorage, because
  // the OAuth callback always comes back to bare /admin.
  let wanted: string | null = null;

  const dirty = $derived(view.status === 'panel' && panelDirty);
  const open = $derived.by(() => {
    if (view.status !== 'panel') return null;
    const id = view.guildId;
    return guilds.find((g) => g.guild_id === id) ?? null;
  });

  function guildName(g: AdminGuild): string {
    return g.name ?? `Server ${g.guild_id}`;
  }

  function urlFor(next: View): string {
    return adminUrl(location.pathname, next.status === 'panel' ? next.guildId : null);
  }

  /** Changes the view. Unsaved settings do not follow: callers have asked first. */
  function show(next: View): void {
    panelDirty = false;
    leaving = null;
    view = next;
  }

  /** A view the page chose (first load, sign-out): the URL is corrected in place. */
  function place(next: View): void {
    show(next);
    history.replaceState(null, '', urlFor(next));
  }

  /** After a change of view: start at its top, with focus on its heading. */
  function arrive(): void {
    window.scrollTo(0, 0);
    void tick().then(() => {
      root?.querySelector<HTMLElement>('h1')?.focus({ preventScroll: true });
    });
  }

  /** A view the admin chose: a new history entry, so Back returns from it. */
  function go(next: View): void {
    show(next);
    history.pushState(null, '', urlFor(next));
    arrive();
  }

  function dropSession(): void {
    storeToken(null);
    api = null;
  }

  onMount(() => {
    // The OAuth callback hands back a token or an error code in the fragment.
    const fragment = parseFragment(location.hash);
    if (fragment) history.replaceState(null, '', location.pathname + location.search);
    wanted = guildInUrl(location.search);

    if (fragment?.kind === 'error') {
      // A sign-in that just failed outranks whatever token is still stored.
      show({ status: 'login', problem: signInProblem(fragment.code, location.origin) });
      return;
    }
    if (fragment?.kind === 'token') {
      storeToken(fragment.token);
      wanted ??= takeRememberedGuild();
    }
    const token = fragment?.kind === 'token' ? fragment.token : readToken();
    if (!token) {
      show({ status: 'login', problem: null });
      return;
    }
    api = new AdminApi(token);
    void loadGuilds();
  });

  async function loadGuilds(): Promise<void> {
    if (!api) return;
    show({ status: 'loading' });
    try {
      guilds = await api.listGuilds();
    } catch (e) {
      if (isUnauthorized(e)) expire(false);
      else show({ status: 'error', message: adminErrorMessage(e), retry: canRetry(e) });
      return;
    }
    const target = wanted;
    wanted = null;
    const [only] = guilds;
    if (!only) {
      dropSession();
      show({ status: 'login', problem: NO_GUILDS });
    } else if (target !== null && guilds.some((g) => g.guild_id === target)) {
      place({ status: 'panel', guildId: target });
    } else if (target !== null) {
      place({
        status: 'guilds',
        note: 'That link is for a server this sign-in can’t manage. Choose one below, or sign out and sign in with another Discord account.',
      });
    } else if (guilds.length === 1) {
      place({ status: 'panel', guildId: only.guild_id });
    } else {
      place({ status: 'guilds', note: null });
    }
  }

  /** Back or Forward moved the URL: show what it names. */
  function onPopState(): void {
    if (!api || (view.status !== 'guilds' && view.status !== 'panel')) return;
    const target = guildInUrl(location.search);
    const here = view.status === 'panel' ? view.guildId : null;
    if (target === here) return;
    if (dirty) {
      // Back was pressed over unsaved settings. Put this panel's entry back,
      // ask, and only go once the admin says so.
      history.pushState(null, '', urlFor(view));
      leave(() => history.back());
      return;
    }
    const [only] = guilds;
    if (target !== null && guilds.some((g) => g.guild_id === target)) {
      show({ status: 'panel', guildId: target });
    } else if (guilds.length === 1 && only) {
      place({ status: 'panel', guildId: only.guild_id });
    } else {
      place({ status: 'guilds', note: null });
    }
    arrive();
  }

  /** Runs `action` now, or asks first when settings are unsaved. */
  function leave(action: () => void): void {
    if (!dirty) {
      action();
      return;
    }
    asker = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    leaving = action;
    void tick().then(() => guard?.focus());
  }

  function keepEditing(): void {
    leaving = null;
    // Safari does not focus a button on click, so there may be nothing to
    // hand focus back to: the panel's heading is the fallback.
    if (asker && asker !== document.body && asker.isConnected) asker.focus();
    else root?.querySelector<HTMLElement>('h1')?.focus({ preventScroll: true });
  }

  function discardAndLeave(): void {
    const action = leaving;
    leaving = null;
    panelDirty = false;
    action?.();
  }

  function signOut(): void {
    dropSession();
    forgetTab();
    guilds = [];
    wanted = null;
    place({ status: 'login', problem: null });
    arrive();
  }

  /** The server stopped accepting the token: sign in again, then come back here. */
  function expire(draftKept: boolean): void {
    if (view.status === 'panel') wanted = view.guildId;
    dropSession();
    show({ status: 'login', problem: sessionExpired(draftKept) });
    arrive();
  }

  /** A server this sign-in no longer covers: a new sign-in refreshes the list. */
  function signInAgain(): void {
    if (view.status === 'panel') wanted = view.guildId;
    dropSession();
    show({
      status: 'login',
      problem: {
        title: 'Sign in again',
        message: 'A new sign-in refreshes the list of servers you can manage.',
      },
    });
    arrive();
  }

  /** On the way to Discord: remember where to come back to. */
  function beforeSignIn(): void {
    if (wanted !== null) rememberGuild(wanted);
  }

  // Closing or reloading the tab over unsaved settings gets the browser's own prompt.
  $effect(() => {
    if (!dirty) return;
    const warn = (e: BeforeUnloadEvent): void => {
      e.preventDefault();
      // Older browsers only prompt when this is set.
      e.returnValue = '';
    };
    window.addEventListener('beforeunload', warn);
    return () => window.removeEventListener('beforeunload', warn);
  });
</script>

<svelte:window onpopstate={onPopState} />

<main class="admin" bind:this={root}>
  <header class="bar">
    <span class="brand">🍃 leaf admin</span>
    {#if view.status === 'guilds' || view.status === 'panel'}
      <div class="right">
        {#if view.status === 'panel' && guilds.length > 1}
          <Button size="sm" onclick={() => leave(() => go({ status: 'guilds', note: null }))}>
            Switch server
          </Button>
        {/if}
        <Button size="sm" variant="ghost" onclick={() => leave(signOut)}>Sign out</Button>
      </div>
    {/if}
    {#if leaving}
      <div
        class="guard"
        role="group"
        aria-labelledby="leave-question"
        tabindex="-1"
        bind:this={guard}
      >
        <p id="leave-question">
          This server’s settings have unsaved changes. Leave without saving them?
        </p>
        <div class="choices">
          <Button size="sm" variant="danger" onclick={discardAndLeave}>Discard changes</Button>
          <Button size="sm" onclick={keepEditing}>Keep editing</Button>
        </div>
      </div>
    {/if}
  </header>

  {#if view.status === 'loading'}
    <div class="waiting">
      <Spinner label="Loading your servers" />
    </div>
  {:else if view.status === 'login'}
    <section class="card">
      {#if view.problem}
        <h1 tabindex="-1">{view.problem.title}</h1>
        <p>{view.problem.message}</p>
      {:else}
        <h1 tabindex="-1">Manage leaf in your server</h1>
        <p>Sign in with Discord to change your server’s leaf settings and manage its series.</p>
      {/if}
      <a class="signin" href="/admin/login" onclick={beforeSignIn}>
        {view.problem ? 'Sign in again' : 'Sign in with Discord'}
      </a>
    </section>
  {:else if view.status === 'error'}
    <div class="failed">
      <ErrorState
        title="Couldn’t load your servers"
        message={view.message}
        onRetry={view.retry ? () => void loadGuilds() : undefined}
        onBack={signOut}
        backLabel="Sign out"
      />
    </div>
  {:else if view.status === 'guilds'}
    <section class="list">
      <h1 tabindex="-1">Choose a server</h1>
      {#if view.note}<Callout tone="warning">{view.note}</Callout>{/if}
      <ul class="guilds">
        {#each guilds as g (g.guild_id)}
          <li>
            <button
              type="button"
              class="guild"
              onclick={() => go({ status: 'panel', guildId: g.guild_id })}
            >
              <GuildIcon name={guildName(g)} url={g.icon_url} />
              <span class="guild-name">{guildName(g)}</span>
              <span class="count">{g.series_count} series</span>
            </button>
          </li>
        {/each}
      </ul>
    </section>
  {:else if view.status === 'panel' && api}
    {#key view.guildId}
      <GuildPanel
        {api}
        guildId={view.guildId}
        name={open?.name}
        iconUrl={open?.icon_url}
        onUnauthorized={expire}
        onDirtyChange={(isDirty) => (panelDirty = isDirty)}
        onBack={guilds.length > 1 ? () => go({ status: 'guilds', note: null }) : undefined}
        onSignIn={signInAgain}
      />
    {/key}
  {/if}
</main>

<style>
  .admin {
    width: 100%;
    max-width: 56rem;
    margin: 0 auto;
    padding: 0 var(--space-md) var(--space-xl);
  }
  /* Sticky, and wrapping: on a narrow phone the buttons drop under the brand
   * instead of pushing the page wider. The safe-area recipe from app.css
   * keeps the bar's ground over the top inset once it is stuck. */
  .bar {
    position: sticky;
    top: 0;
    z-index: 2;
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-xs) var(--space-sm);
    align-items: center;
    justify-content: space-between;
    min-height: var(--appbar-h);
    margin-top: calc(-1 * var(--safe-top));
    padding: calc(var(--safe-top) + var(--space-xs)) 0 var(--space-xs);
    background: var(--canvas);
    border-bottom: 1px solid var(--hairline);
  }
  .brand {
    font-family: var(--font-display);
    font-size: var(--fs-body);
    font-weight: var(--fw-display);
    white-space: nowrap;
  }
  .right,
  .choices {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-xs);
  }
  .guard {
    display: grid;
    flex: 1 1 100%;
    gap: var(--space-xs);
    padding: var(--space-sm);
    background: var(--surface-1);
    border: 1px solid var(--hairline-strong);
    border-left: 2px solid var(--warning-fill);
    border-radius: var(--radius-md);
    outline: none;
  }
  .guard p {
    margin: 0;
    font-size: var(--fs-body-sm);
  }
  .waiting {
    display: grid;
    place-items: center;
    padding: var(--space-xxl) 0;
  }
  .card {
    display: grid;
    gap: var(--space-md);
    justify-items: center;
    max-width: 30rem;
    margin: var(--space-xl) auto 0;
    padding: var(--space-xl) var(--space-lg);
    text-align: center;
    background: var(--surface-1);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-xl);
    box-shadow: var(--shadow-card);
  }
  .card p {
    margin: 0;
    color: var(--ink-muted);
  }
  h1 {
    margin: 0;
    font-size: var(--fs-card-title);
    font-weight: var(--fw-display);
    letter-spacing: var(--tracking-display);
    line-height: 1.2;
  }
  /* Focused by script when the view changes; it is not a control. */
  h1:focus {
    outline: none;
  }
  /* A link, because it is one: it leaves for Discord's sign-in. Drawn as the
   * primary button. */
  .signin,
  .signin:visited {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-height: var(--control-height);
    padding: 0 26px;
    color: var(--inverse-ink);
    font-weight: var(--fw-display);
    text-decoration: none;
    background: var(--inverse-canvas);
    border-radius: var(--radius-pill);
    box-shadow: var(--shadow-soft);
  }
  .signin:active {
    transform: scale(0.98);
  }
  .failed {
    margin-top: var(--space-xl);
  }
  .list {
    display: grid;
    gap: var(--space-md);
    margin-top: var(--space-lg);
  }
  .guilds {
    display: grid;
    gap: var(--space-sm);
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .guild {
    display: flex;
    gap: var(--space-sm);
    align-items: center;
    width: 100%;
    min-height: var(--touch-target);
    padding: var(--space-sm) var(--space-md);
    color: var(--ink);
    font: inherit;
    text-align: left;
    background: var(--surface-1);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-xl);
    box-shadow: var(--shadow-soft);
    cursor: pointer;
  }
  .guild:active {
    background: var(--surface-2);
  }
  .guild-name {
    flex: 1 1 auto;
    min-width: 0;
    font-weight: var(--fw-emphasis);
    overflow-wrap: anywhere;
  }
  .count {
    flex: none;
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
  }

  /* 360px phones: slimmer buttons keep the brand and both actions on one
   * line, so the sticky bar stays one row tall. */
  @media (max-width: 399px) {
    .right :global(.btn) {
      padding: 0 var(--space-sm);
    }
  }
  @media (min-width: 560px) {
    .brand {
      font-size: var(--fs-subhead);
    }
  }
  @media (hover: hover) {
    .signin:hover {
      filter: brightness(1.04);
    }
    .guild:hover {
      background: var(--surface-2);
    }
  }
</style>
