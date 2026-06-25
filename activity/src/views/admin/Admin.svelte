<script lang="ts">
  import { onMount } from 'svelte';

  import { AdminApi, AdminApiError } from '../../lib/admin/client';
  import type { AdminGuild } from '../../lib/admin/schemas';
  import GuildPanel from './GuildPanel.svelte';

  const TOKEN_KEY = 'leaf:adminToken';

  type View =
    | { status: 'loading' }
    | { status: 'login' }
    | { status: 'guilds'; guilds: AdminGuild[] }
    | { status: 'panel'; guildId: string }
    | { status: 'error'; message: string };

  let view = $state<View>({ status: 'loading' });
  // Reassigned on login/logout; the panel branch reads it, so it must be state.
  let api = $state<AdminApi | null>(null);

  onMount(() => {
    // The OAuth callback hands the token back in the URL fragment.
    if (location.hash.startsWith('#token=')) {
      const token = decodeURIComponent(location.hash.slice('#token='.length));
      localStorage.setItem(TOKEN_KEY, token);
      history.replaceState(null, '', location.pathname);
    }
    const stored = localStorage.getItem(TOKEN_KEY);
    if (!stored) {
      view = { status: 'login' };
      return;
    }
    api = new AdminApi(stored);
    void loadGuilds();
  });

  async function loadGuilds(): Promise<void> {
    if (!api) return;
    view = { status: 'loading' };
    try {
      const guilds = await api.listGuilds();
      const [only] = guilds;
      if (guilds.length === 0) {
        view = { status: 'error', message: 'You don’t manage a server that has leaf.' };
      } else if (guilds.length === 1 && only) {
        view = { status: 'panel', guildId: only.guild_id };
      } else {
        view = { status: 'guilds', guilds };
      }
    } catch (e) {
      if (e instanceof AdminApiError && e.status === 401) {
        signOut();
      } else {
        view = { status: 'error', message: e instanceof Error ? e.message : String(e) };
      }
    }
  }

  function signOut(): void {
    localStorage.removeItem(TOKEN_KEY);
    api = null;
    view = { status: 'login' };
  }
  function signIn(): void {
    location.href = '/admin/login';
  }
</script>

<main class="admin">
  <header class="bar">
    <span class="brand">🍃 leaf admin</span>
    {#if view.status === 'guilds' || view.status === 'panel'}
      <div class="right">
        {#if view.status === 'panel'}
          <button class="ghost" onclick={() => void loadGuilds()}>Switch server</button>
        {/if}
        <button class="ghost" onclick={signOut}>Sign out</button>
      </div>
    {/if}
  </header>

  {#if view.status === 'loading'}
    <p class="muted pad">Loading…</p>
  {:else if view.status === 'login'}
    <div class="card center">
      <p>Sign in with Discord to manage your server’s leaf settings and series.</p>
      <button class="primary" onclick={signIn}>Sign in with Discord</button>
    </div>
  {:else if view.status === 'error'}
    <div class="card center">
      <p>{view.message}</p>
      <button class="ghost" onclick={signOut}>Sign out</button>
    </div>
  {:else if view.status === 'guilds'}
    <ul class="guilds">
      {#each view.guilds as g (g.guild_id)}
        <li>
          <button class="guild" onclick={() => (view = { status: 'panel', guildId: g.guild_id })}>
            <span>Server {g.guild_id}</span>
            <span class="muted">{g.series_count} series</span>
          </button>
        </li>
      {/each}
    </ul>
  {:else if view.status === 'panel' && api}
    <GuildPanel {api} guildId={view.guildId} />
  {/if}
</main>

<style>
  .admin {
    width: 100%;
    max-width: 56rem;
    margin: 0 auto;
    padding: var(--space-md);
  }
  .bar {
    position: sticky;
    top: 0;
    z-index: 1;
    display: flex;
    align-items: center;
    justify-content: space-between;
    min-height: var(--appbar-h);
    background: var(--canvas);
    border-bottom: 1px solid var(--hairline);
  }
  .brand {
    font-family: var(--font-display);
    font-size: var(--fs-subhead);
    font-weight: var(--fw-display);
  }
  .right {
    display: flex;
    gap: var(--space-xs);
  }
  .pad {
    padding: var(--space-lg);
  }
  .muted {
    color: var(--ink-muted);
  }
  .card {
    margin-top: var(--space-xl);
    padding: var(--space-xl);
    background: var(--surface-1);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-xl);
    box-shadow: var(--shadow-card);
  }
  .center {
    display: grid;
    gap: var(--space-md);
    justify-items: center;
    text-align: center;
  }
  .guilds {
    display: grid;
    gap: var(--space-sm);
    margin: var(--space-lg) 0 0;
    padding: 0;
    list-style: none;
  }
  .guild {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    padding: var(--space-md);
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
  .primary {
    padding: 12px 22px;
    color: var(--inverse-ink);
    font: inherit;
    font-weight: var(--fw-display);
    background: var(--inverse-canvas);
    border: 0;
    border-radius: var(--radius-pill);
    box-shadow: var(--shadow-soft);
    cursor: pointer;
  }
  .ghost {
    padding: 8px 14px;
    color: var(--ink);
    font: inherit;
    font-size: var(--fs-body-sm);
    font-weight: var(--fw-emphasis);
    background: var(--surface-2);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-pill);
    cursor: pointer;
  }
</style>
