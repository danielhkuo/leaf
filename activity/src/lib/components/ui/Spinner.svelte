<script lang="ts">
  // Minimal ring spinner. Under prefers-reduced-motion the ring stops turning
  // and breathes instead (opacity only), so it still reads as "working"
  // rather than as a frozen frame.
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
  @keyframes breathe {
    to {
      opacity: 0.35;
    }
  }
  /* !important because app.css's reduced-motion reset is !important too: it
   * would cut this to a single 0.01ms run and leave a static ring. */
  @media (prefers-reduced-motion: reduce) {
    .spinner {
      animation: breathe 1.2s ease-in-out infinite alternate !important;
    }
  }
</style>
