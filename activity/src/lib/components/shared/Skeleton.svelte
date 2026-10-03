<script lang="ts">
  // A placeholder block shown while content loads. It is a status with a
  // visually hidden label, so assistive tech has text to read for it. A view
  // that shows several blocks at once passes `label=""` on all but one: those
  // are then decorative and the screen says "Loading" once.
  interface Props {
    width?: string;
    height?: string;
    radius?: string;
    /** What is loading, for screen readers. Empty hides the block from them. */
    label?: string;
  }
  let {
    width = '100%',
    height = '1rem',
    radius = 'var(--radius-sm)',
    label = 'Loading',
  }: Props = $props();
</script>

<div
  class="skeleton"
  style="width:{width};height:{height};border-radius:{radius}"
  role={label ? 'status' : undefined}
  aria-hidden={label ? undefined : 'true'}
>
  {#if label}<span class="sr-only">{label}</span>{/if}
</div>

<style>
  .skeleton {
    position: relative;
    overflow: hidden;
    background: var(--surface-2);
  }
  .skeleton::after {
    content: '';
    position: absolute;
    inset: 0;
    background: linear-gradient(90deg, transparent, var(--surface-3), transparent);
    transform: translateX(-100%);
    animation: shimmer 1.4s infinite;
  }
  @keyframes shimmer {
    100% {
      transform: translateX(100%);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .skeleton::after {
      animation: none;
    }
  }
</style>
