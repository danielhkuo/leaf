<script lang="ts">
  // Dev-only gallery of every leaf screen (reach it at /mock.html via
  // `npm run dev`). The sidebar picks a screen; the stage embeds it in an
  // iframe sized to the chosen device width, so each screen's own media
  // queries respond to a real viewport width (true phone vs. desktop preview).
  // `?embed=1&screen=<id>` renders just that screen — that's what the iframe
  // loads, so all rendering logic lives in Screen.svelte. `&long=1` fills it
  // with worst-case text (the longest names leaf allows, with no spaces).
  import Screen from './Screen.svelte';
  import { SCREEN_GROUPS, SCREENS, type ScreenId } from './screens';

  const params = new URLSearchParams(location.search);
  const embed = params.has('embed');
  const embedScreen = params.get('screen') ?? 'picker';
  const embedLongText = params.has('long');

  // Phone widths worth checking: the narrowest iPhone and Android layouts,
  // the common iPhone width, and a large phone. `null` fills the stage.
  const widths = [
    { label: '320', px: 320 },
    { label: '375', px: 375 },
    { label: '430', px: 430 },
    { label: 'Wide', px: null },
  ];

  let current = $state<ScreenId>('picker');
  let width = $state<number | null>(375);
  let longText = $state(false);
  const label = $derived(SCREENS.find((s) => s.id === current)?.label ?? current);
</script>

{#if embed}
  <Screen id={embedScreen} longText={embedLongText} />
{:else}
  <div class="shell">
    <aside class="nav">
      <div class="brand">🍃 leaf <span>screens</span></div>
      <div class="screens">
        {#each SCREEN_GROUPS as group (group.name)}
          <p class="group">{group.name}</p>
          {#each group.items as item (item.id)}
            <button
              type="button"
              class="link"
              class:active={current === item.id}
              aria-current={current === item.id ? 'page' : undefined}
              onclick={() => (current = item.id)}
            >
              {item.label}
            </button>
          {/each}
        {/each}
      </div>
      <label class="toggle">
        <input type="checkbox" bind:checked={longText} />
        Long text
      </label>
      <div class="width" role="group" aria-label="Preview width">
        {#each widths as w (w.label)}
          <button
            type="button"
            class:on={width === w.px}
            aria-pressed={width === w.px}
            onclick={() => (width = w.px)}
          >
            {w.label}
          </button>
        {/each}
      </div>
    </aside>

    <main class="stage" class:phone={width !== null}>
      <iframe
        class="device"
        class:framed={width !== null}
        style:width={width === null ? null : `${width}px`}
        title={label}
        src="/mock.html?embed=1&screen={current}{longText ? '&long=1' : ''}"
      ></iframe>
    </main>
  </div>
{/if}

<style>
  /* The viewport's height, not 100%: `#app` grows with its content, so a
   * percentage would let a long sidebar scroll the page instead of itself. */
  .shell {
    display: grid;
    grid-template-columns: 232px minmax(0, 1fr);
    height: 100dvh;
  }

  /* Viewer chrome — deliberately dark, so it never reads as part of the app. */
  .nav {
    display: flex;
    flex-direction: column;
    min-height: 0;
    padding: 16px 12px;
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
  /* The list scrolls by itself, so the width and text switches under it
   * stay in reach on a short window. */
  .screens {
    display: flex;
    flex: 1 1 auto;
    flex-direction: column;
    gap: 2px;
    min-height: 0;
    overflow-y: auto;
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
  .toggle {
    display: flex;
    gap: 8px;
    align-items: center;
    margin-top: 8px;
    padding: 8px;
    cursor: pointer;
  }
  /* The app's ink-coloured tick would vanish on this dark ground. */
  .toggle input {
    accent-color: #72a4f2;
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
    min-height: 0;
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
  /* The frame's border is outside the simulated width (content-box), so the
   * screen inside sees exactly the chosen number of pixels. */
  .device.framed {
    box-sizing: content-box;
    max-width: 100%;
    height: calc(100% - 2px);
    border: 1px solid rgba(32, 32, 32, 0.18);
    border-radius: 28px;
    box-shadow: 0 24px 60px rgba(32, 32, 32, 0.18);
    overflow: hidden;
  }
</style>
