<script lang="ts">
  // Renders a single leaf screen by id with fixture data and no Discord SDK /
  // network / auth. The gallery and creator screens are the real views over a
  // mock API (MockGallery); the admin panel is the real GuildPanel with a
  // stand-in client. Only what cannot run in a browser tab is mirrored: the
  // boot screens (MockBoot) and the admin page's own chrome, below.
  // ScreenViewer embeds this per screen inside a width-controlled iframe, so
  // each screen sees a real device-width viewport for its media queries.
  import { untrack } from 'svelte';

  import Button from '../lib/components/ui/Button.svelte';
  import type { View } from '../lib/stores/nav.svelte';
  import GuildPanel from '../views/admin/GuildPanel.svelte';
  import Landing from '../views/Landing.svelte';
  import type { Scenario } from './api';
  import {
    bootScreens,
    createMockAdminApi,
    eligibilityBlocked,
    eligibilityOk,
    guildDetail,
    homeSeries,
    MISSING_DAY,
    optionsChannelUnseen,
    series,
    seriesChannelGone,
    VIDEO_DAY,
    VIEWER_DAY,
    worstCase,
  } from './fixtures';
  import MockBoot from './MockBoot.svelte';
  import MockGallery from './MockGallery.svelte';
  import type { ScreenId } from './screens';

  interface Props {
    id: string;
    /** Show every name, description and caption at its worst-case length. */
    longText?: boolean;
  }
  let { id, longText = false }: Props = $props();

  interface GalleryScreen {
    scenario?: Scenario;
    stack: [View, ...View[]];
  }

  const PICKER: View = { name: 'picker' };
  const home = (seriesId: number): View => ({ name: 'home', seriesId });
  const viewer = (day: number): View => ({ name: 'viewer', seriesId: homeSeries.id, day });
  /** What a member who has not started a series sees. */
  const others = series.filter((s) => !s.is_owner);

  // Keyed by ids the sidebar lists (screens.ts), so none is out of reach.
  const GALLERY: ReadonlyMap<string, GalleryScreen> = new Map([
    ['picker', { stack: [PICKER] }],
    [
      'picker-empty',
      {
        scenario: { series: [], eligibility: { ...eligibilityOk, owns_any: false } },
        stack: [PICKER],
      },
    ],
    [
      'picker-blocked',
      { scenario: { series: others, eligibility: eligibilityBlocked }, stack: [PICKER] },
    ],
    ['home', { stack: [PICKER, home(homeSeries.id)] }],
    // A series the viewer has just started: nothing archived yet.
    ['home-empty', { stack: [PICKER, home(8)] }],
    // The viewer's own sprout, two days in.
    ['home-sprout', { stack: [PICKER, home(1)] }],
    // The viewer's own series after its channel was deleted in Discord.
    [
      'home-channel-gone',
      { scenario: { series: seriesChannelGone }, stack: [PICKER, home(homeSeries.id)] },
    ],
    ['viewer', { stack: [PICKER, home(homeSeries.id), viewer(VIEWER_DAY)] }],
    ['viewer-video', { stack: [PICKER, home(homeSeries.id), viewer(VIDEO_DAY)] }],
    [
      'viewer-loading',
      { scenario: { holdDays: true }, stack: [PICKER, home(homeSeries.id), viewer(VIEWER_DAY)] },
    ],
    // A day the server does not have (removed since the calendar loaded, say).
    ['viewer-failed', { stack: [PICKER, home(homeSeries.id), viewer(MISSING_DAY)] }],
    ['create', { stack: [PICKER, { name: 'createSeries' }] }],
    [
      'create-blocked',
      {
        scenario: { series: others, eligibility: eligibilityBlocked },
        stack: [PICKER, { name: 'createSeries' }],
      },
    ],
    ['myseries', { stack: [PICKER, { name: 'mySeries' }] }],
    [
      'settings',
      {
        stack: [PICKER, { name: 'mySeries' }, { name: 'seriesSettings', seriesId: homeSeries.id }],
      },
    ],
    // Where "Choose another channel" leads from home-channel-gone while no
    // admin has run /setup: the deleted channel is the only one on offer.
    [
      'settings-channel-unseen',
      {
        scenario: { series: seriesChannelGone, options: optionsChannelUnseen },
        stack: [PICKER, home(homeSeries.id), { name: 'seriesSettings', seriesId: homeSeries.id }],
      },
    ],
    // A link to a series this viewer can't see (hidden, removed, or never there).
    ['unavailable', { stack: [PICKER, home(4242)] }],
    ['expired', { scenario: { listFails: 'expired' }, stack: [PICKER] }],
    ['load-error', { scenario: { listFails: 'unavailable' }, stack: [PICKER] }],
    // Minimised: the same screens, in a viewport the size of Discord's tile
    // (the screen viewer and the suites open them at one). The size is what
    // turns them into the card; at a phone's they are the screens above.
    ['tile-day', { stack: [PICKER, home(homeSeries.id), viewer(VIEWER_DAY)] }],
    ['tile-series', { stack: [PICKER, home(homeSeries.id)] }],
    ['tile-empty', { stack: [PICKER, home(8)] }],
    ['tile-list', { stack: [PICKER] }],
    ['tile-expired', { scenario: { listFails: 'expired' }, stack: [PICKER] }],
  ] satisfies [ScreenId, GalleryScreen][]);

  const gallery = $derived(GALLERY.get(id));
  const boot = $derived(bootScreens.get(id));
  // One screen per page (see ScreenViewer), so the switch is read once.
  const long = untrack(() => longText);
  const adminApi = createMockAdminApi(long);
  const guild = long ? worstCase(guildDetail) : guildDetail;
  const noop = (): void => undefined;
</script>

{#if gallery}
  <MockGallery scenario={{ ...gallery.scenario, longText: long }} stack={gallery.stack} />
{:else if boot}
  <MockBoot {boot} />
{:else if id === 'landing'}
  <Landing />
{:else if id === 'admin-login'}
  <main class="admin">
    <header class="bar"><span class="brand">🍃 leaf admin</span></header>
    <section class="card">
      <h1 tabindex="-1">Manage leaf in your server</h1>
      <p>Sign in with Discord to change your server’s leaf settings and manage its series.</p>
      <!-- The real link leaves for Discord's sign-in; the mock stays put. -->
      <a class="signin" href="/admin/login" onclick={(e) => e.preventDefault()}>
        Sign in with Discord
      </a>
    </section>
  </main>
{:else if id === 'admin-panel'}
  <main class="admin">
    <header class="bar">
      <span class="brand">🍃 leaf admin</span>
      <div class="right">
        <Button size="sm" onclick={noop}>Switch server</Button>
        <Button size="sm" variant="ghost" onclick={noop}>Sign out</Button>
      </div>
    </header>
    <GuildPanel api={adminApi} guildId={guild.guild_id} name={guild.name} onBack={noop} />
  </main>
{:else}
  <p class="unknown">There is no mock screen called “{id}”.</p>
{/if}

<style>
  /* The admin page's chrome, mirrored from views/admin/Admin.svelte (its
   * header and sign-in card), which needs a stored token to mount. */
  .admin {
    width: 100%;
    max-width: 56rem;
    margin: 0 auto;
    padding: 0 var(--space-md) var(--space-xl);
  }
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
  .right {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-xs);
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
  h1:focus {
    outline: none;
  }
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
  }

  .unknown {
    margin: var(--space-xl) var(--space-md);
    color: var(--ink-muted);
    text-align: center;
  }
</style>
