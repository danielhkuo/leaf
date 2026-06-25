<script lang="ts">
  // Dev-only gallery of every leaf screen (reach it at /mock.html via
  // `npm run dev`). The sidebar picks a screen; the stage embeds it in an
  // iframe sized to the chosen device width, so each screen's own media
  // queries respond to a real viewport width (true phone vs. desktop preview).
  // `?embed=1&screen=<id>` renders just that screen — that's what the iframe
  // loads, so all rendering logic lives in Screen.svelte.
  import Screen from './Screen.svelte';

  const params = new URLSearchParams(location.search);
  const embed = params.has('embed');
  const embedScreen = params.get('screen') ?? 'picker';

  const groups = [
    {
      name: 'Gallery',
      items: [
        { id: 'picker', label: 'Series picker' },
        { id: 'picker-empty', label: 'Picker — empty' },
        { id: 'picker-blocked', label: 'Picker — blocked' },
        { id: 'home', label: 'Series home' },
        { id: 'home-empty', label: 'Home — sprouting' },
        { id: 'viewer', label: 'Day viewer' },
      ],
    },
    {
      name: 'Creator',
      items: [
        { id: 'create', label: 'Create series' },
        { id: 'myseries', label: 'My series' },
        { id: 'settings', label: 'Series settings' },
      ],
    },
    {
      name: 'Admin',
      items: [
        { id: 'admin-login', label: 'Admin login' },
        { id: 'admin-panel', label: 'Admin panel' },
      ],
    },
    {
      name: 'States',
      items: [
        { id: 'loading', label: 'Loading' },
        { id: 'error', label: 'Error' },
      ],
    },
  ];

  let current = $state('picker');
  let wide = $state(false);
  const label = $derived(
    groups.flatMap((g) => g.items).find((i) => i.id === current)?.label ?? current,
  );
</script>

{#if embed}
  <Screen id={embedScreen} />
{:else}
  <div class="shell">
    <aside class="nav">
      <div class="brand">🍃 leaf <span>screens</span></div>
      {#each groups as group (group.name)}
        <p class="group">{group.name}</p>
        {#each group.items as item (item.id)}
          <button
            class="link"
            class:active={current === item.id}
            onclick={() => (current = item.id)}
          >
            {item.label}
          </button>
        {/each}
      {/each}
      <div class="spacer"></div>
      <div class="width">
        <button class:on={!wide} onclick={() => (wide = false)}>Phone</button>
        <button class:on={wide} onclick={() => (wide = true)}>Wide</button>
      </div>
    </aside>

    <main class="stage" class:phone={!wide}>
      <iframe
        class="device"
        class:framed={!wide}
        title={label}
        src="/mock.html?embed=1&screen={current}"
      ></iframe>
    </main>
  </div>
{/if}

<style>
  .shell {
    display: grid;
    grid-template-columns: 232px 1fr;
    height: 100%;
  }

  /* Viewer chrome — deliberately dark, so it never reads as part of the app. */
  .nav {
    display: flex;
    flex-direction: column;
    gap: 2px;
    height: 100%;
    padding: 16px 12px;
    overflow-y: auto;
    color: #d9d5cd;
    background: #201f1d;
    font-family:
      system-ui,
      -apple-system,
      sans-serif;
    font-size: 13px;
  }
  .brand {
    margin-bottom: 12px;
    padding: 0 8px;
    color: #fff;
    font-size: 16px;
    font-weight: 700;
  }
  .brand span {
    color: #8a857c;
    font-weight: 500;
  }
  .group {
    margin: 14px 8px 4px;
    color: #8a857c;
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .link {
    padding: 7px 8px;
    color: #d9d5cd;
    font: inherit;
    text-align: left;
    background: transparent;
    border: 0;
    border-radius: 7px;
    cursor: pointer;
  }
  .link:hover {
    background: #2c2b28;
  }
  .link.active {
    color: #201f1d;
    font-weight: 600;
    background: #72a4f2;
  }
  .spacer {
    flex: 1;
  }
  .width {
    display: flex;
    gap: 4px;
    padding: 8px 4px 0;
  }
  .width button {
    flex: 1;
    padding: 6px;
    color: #d9d5cd;
    font: inherit;
    background: #2c2b28;
    border: 0;
    border-radius: 7px;
    cursor: pointer;
  }
  .width button.on {
    color: #201f1d;
    font-weight: 600;
    background: #d9d5cd;
  }

  /* Stage — hosts the iframe whose width is the simulated device width. */
  .stage {
    height: 100%;
    overflow: hidden;
    background: #ece6db;
  }
  .stage.phone {
    display: grid;
    place-items: start center;
    padding: 24px;
  }
  .device {
    width: 100%;
    height: 100%;
    background: var(--canvas);
    border: 0;
  }
  .device.framed {
    width: 430px;
    max-width: 100%;
    height: calc(100% - 48px);
    border: 1px solid rgba(32, 32, 32, 0.18);
    border-radius: 28px;
    box-shadow: 0 24px 60px rgba(32, 32, 32, 0.18);
    overflow: hidden;
  }
</style>
