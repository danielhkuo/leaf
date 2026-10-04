<script lang="ts">
  // A server's icon, or its initial on a tinted disc when it has none (or
  // the image doesn't load). Decorative: the name is always next to it.
  interface Props {
    name: string;
    url?: string | undefined;
    /** Width and height in CSS pixels. */
    size?: number;
  }
  let { name, url, size = 40 }: Props = $props();

  /** The URL that failed, so a new one gets its own try. */
  let broken = $state<string | undefined>(undefined);
  const initial = $derived(Array.from(name.trim())[0]?.toUpperCase() ?? '🍃');
</script>

{#if url && url !== broken}
  <img
    class="icon"
    src={url}
    alt=""
    width={size}
    height={size}
    loading="lazy"
    decoding="async"
    referrerpolicy="no-referrer"
    onerror={() => (broken = url)}
  />
{:else}
  <span class="icon blank" style="width:{size}px;height:{size}px" aria-hidden="true">{initial}</span
  >
{/if}

<style>
  .icon {
    flex: none;
    border-radius: 50%;
    object-fit: cover;
  }
  .blank {
    display: inline-grid;
    place-items: center;
    color: var(--ink);
    font-weight: var(--fw-display);
    background: var(--surface-3);
  }
</style>
