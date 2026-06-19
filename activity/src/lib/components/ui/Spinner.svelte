<script lang="ts">
  // Minimal ring spinner — transform-only animation, stops under
  // prefers-reduced-motion (the global rule neutralizes the spin).
  interface Props {
    size?: string;
    label?: string;
  }
  let { size = '28px', label = 'Loading' }: Props = $props();
</script>

<span class="spinner" style="--spinner-size:{size}" role="status" aria-label={label}></span>

<style>
  .spinner {
    display: inline-block;
    width: var(--spinner-size);
    height: var(--spinner-size);
    border: 3px solid var(--hairline-strong);
    border-top-color: var(--ink);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
