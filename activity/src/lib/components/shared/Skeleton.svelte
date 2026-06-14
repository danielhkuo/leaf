<script lang="ts">
  interface Props {
    width?: string;
    height?: string;
    radius?: string;
  }
  let { width = '100%', height = '1rem', radius = 'var(--radius-sm)' }: Props = $props();
</script>

<div class="skeleton" style="width:{width};height:{height};border-radius:{radius}"></div>

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
