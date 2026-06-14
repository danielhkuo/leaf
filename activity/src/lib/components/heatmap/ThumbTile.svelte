<script lang="ts">
  interface Props {
    day: number;
    thumbUrl: string | null;
    onOpen: (day: number) => void;
  }
  let { day, thumbUrl, onOpen }: Props = $props();
</script>

<button class="tile" onclick={() => onOpen(day)} aria-label={`Day ${day}`}>
  {#if thumbUrl}
    <img
      src={thumbUrl}
      alt={`Day ${day}`}
      loading="lazy"
      decoding="async"
      width="120"
      height="120"
    />
  {:else}
    <span class="missing" aria-hidden="true"></span>
  {/if}
  <span class="num">{day}</span>
</button>

<style>
  .tile {
    position: relative;
    display: block;
    padding: 0;
    aspect-ratio: 1;
    background: var(--surface-2);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-md);
    overflow: hidden;
    cursor: pointer;
  }
  img {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .missing {
    display: block;
    width: 100%;
    height: 100%;
    background: repeating-linear-gradient(
      45deg,
      var(--surface-1),
      var(--surface-1) 6px,
      var(--surface-2) 6px,
      var(--surface-2) 12px
    );
  }
  .num {
    position: absolute;
    bottom: 2px;
    right: 4px;
    color: var(--ink);
    font-size: var(--fs-eyebrow);
    text-shadow: 0 1px 2px rgb(0 0 0 / 70%);
  }
</style>
