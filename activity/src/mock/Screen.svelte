<script lang="ts">
  // Renders a single leaf screen by id with fixture data and no Discord SDK /
  // network / auth. Real prop-driven components are used directly; the few
  // store-coupled views are mirrored (MockHome) or fed a stand-in API (admin).
  // ScreenViewer embeds this per screen inside a width-controlled iframe, so
  // each screen sees a real device-width viewport for its media queries.
  import CreateWizard from '../lib/components/creator/CreateWizard.svelte';
  import MySeriesCard from '../lib/components/creator/MySeriesCard.svelte';
  import SeriesSettingsForm from '../lib/components/creator/SeriesSettingsForm.svelte';
  import SeriesPicker from '../lib/components/picker/SeriesPicker.svelte';
  import Button from '../lib/components/ui/Button.svelte';
  import IconButton from '../lib/components/ui/IconButton.svelte';
  import GuildPanel from '../views/admin/GuildPanel.svelte';
  import {
    eligibilityBlocked,
    eligibilityOk,
    mockAdminApi,
    mySeries,
    options,
    series,
    seriesSettings,
  } from './fixtures';
  import MockHome from './MockHome.svelte';
  import MockViewer from './MockViewer.svelte';

  interface Props {
    id: string;
  }
  let { id }: Props = $props();
  const noop = (): void => undefined;
</script>

{#snippet creatorHeader(title: string)}
  <header class="vbar">
    <IconButton ariaLabel="Back" variant="solid" onclick={noop}>←</IconButton>
    <h1 class="vtitle">{title}</h1>
  </header>
{/snippet}

{#if id === 'picker'}
  <SeriesPicker
    {series}
    onSelect={noop}
    eligibility={eligibilityOk}
    eligibilityStatus="ready"
    ownsSeries
    onCreate={noop}
    onManage={noop}
  />
{:else if id === 'picker-empty'}
  <SeriesPicker
    series={[]}
    onSelect={noop}
    eligibility={eligibilityOk}
    eligibilityStatus="ready"
    onCreate={noop}
  />
{:else if id === 'picker-blocked'}
  <SeriesPicker {series} onSelect={noop} eligibility={eligibilityBlocked} eligibilityStatus="ready" />
{:else if id === 'home'}
  <MockHome />
{:else if id === 'home-empty'}
  <MockHome empty />
{:else if id === 'viewer'}
  <MockViewer />
{:else if id === 'create'}
  <div class="view">
    {@render creatorHeader('Start a series')}
    <CreateWizard {options} submitting={false} error={null} onSubmit={noop} />
  </div>
{:else if id === 'myseries'}
  <div class="view">
    {@render creatorHeader('My series')}
    <ul class="list">
      {#each mySeries as s (s.id)}
        <li><MySeriesCard series={s} onOpen={noop} /></li>
      {/each}
    </ul>
  </div>
{:else if id === 'settings'}
  <div class="view">
    {@render creatorHeader(seriesSettings.name)}
    <SeriesSettingsForm
      settings={seriesSettings}
      {options}
      saving={false}
      saved={false}
      error={null}
      onSave={noop}
    />
  </div>
{:else if id === 'admin-login'}
  <main class="admin">
    <header class="abar"><span class="abrand">🍃 leaf admin</span></header>
    <div class="acard">
      <p>Sign in with Discord to manage your server’s leaf settings and series.</p>
      <button class="aprimary">Sign in with Discord</button>
    </div>
  </main>
{:else if id === 'admin-panel'}
  <main class="admin">
    <header class="abar">
      <span class="abrand">🍃 leaf admin</span>
      <span class="aright">
        <button class="aghost">Switch server</button>
        <button class="aghost">Sign out</button>
      </span>
    </header>
    <GuildPanel api={mockAdminApi} guildId="900000000000000009" />
  </main>
{:else if id === 'loading'}
  <div class="boot">
    <div class="center" role="status">
      <span class="leaf" aria-hidden="true">🍃</span>
      <p class="muted">Connecting to Discord…</p>
    </div>
  </div>
{:else if id === 'error'}
  <div class="boot">
    <div class="center">
      <span class="leaf" aria-hidden="true">🍂</span>
      <p>Couldn’t start the gallery.</p>
      <p class="muted small">The Discord handshake timed out.</p>
      <Button variant="primary" onclick={noop}>Try again</Button>
    </div>
  </div>
{/if}

<style>
  /* Reconstructed wrappers (creator header, admin chrome, boot) mirror the
   * real views so each screen reads correctly in isolation. */
  .view {
    display: grid;
    gap: var(--space-md);
    width: 100%;
    max-width: 40rem;
    margin: 0 auto;
    padding: var(--space-md);
  }
  .vbar {
    display: flex;
    gap: var(--space-sm);
    align-items: center;
    min-height: var(--appbar-h);
  }
  .vtitle {
    margin: 0;
    font-size: var(--fs-card-title);
    font-weight: var(--fw-display);
    letter-spacing: var(--tracking-display);
  }
  .list {
    display: grid;
    gap: var(--space-sm);
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .admin {
    width: 100%;
    max-width: 56rem;
    margin: 0 auto;
    padding: var(--space-md);
  }
  .abar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    min-height: var(--appbar-h);
    border-bottom: 1px solid var(--hairline);
  }
  .abrand {
    font-family: var(--font-display);
    font-size: var(--fs-subhead);
    font-weight: var(--fw-display);
  }
  .aright {
    display: flex;
    gap: var(--space-xs);
  }
  .acard {
    display: grid;
    gap: var(--space-md);
    justify-items: center;
    max-width: 28rem;
    margin: var(--space-xl) auto 0;
    padding: var(--space-xl);
    text-align: center;
    background: var(--surface-1);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-xl);
    box-shadow: var(--shadow-card);
  }
  .aprimary {
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
  .aghost {
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

  .boot {
    display: grid;
    place-items: center;
    min-height: 100vh;
    padding: var(--space-lg);
  }
  .center {
    display: grid;
    gap: var(--space-sm);
    justify-items: center;
    text-align: center;
  }
  .leaf {
    font-size: var(--fs-display);
    line-height: 1;
  }
  .muted {
    margin: 0;
    color: var(--ink-muted);
  }
  .small {
    font-size: var(--fs-body-sm);
  }
</style>
