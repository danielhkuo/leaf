<script lang="ts">
  // Blur-up image: shows the (already-cached) thumbnail, scaled and blurred,
  // while the full-res fades in on load. Zero extra network — the thumb came
  // from the grid. Animates opacity only.
  interface Props {
    src: string;
    placeholder?: string;
    alt: string;
  }
  let { src, placeholder, alt }: Props = $props();

  let loaded = $state(false);
  // Reset the fade whenever the source changes (navigating days).
  $effect(() => {
    void src;
    loaded = false;
  });
</script>

<div class="blur">
  {#if placeholder}
    <img class="ph" src={placeholder} alt="" aria-hidden="true" />
  {/if}
  <img class="full" class:loaded {src} {alt} decoding="async" onload={() => (loaded = true)} />
</div>

<style>
  .blur {
    position: relative;
    display: grid;
    place-items: center;
    width: 100%;
    height: 100%;
  }
  .ph,
  .full {
    grid-area: 1 / 1;
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
  }
  .ph {
    filter: blur(16px);
    transform: scale(1.04);
  }
  .full {
    opacity: 0;
    transition: opacity var(--motion-base) var(--ease);
  }
  .full.loaded {
    opacity: 1;
  }
  @media (prefers-reduced-motion: reduce) {
    .full {
      transition: none;
    }
  }
</style>
