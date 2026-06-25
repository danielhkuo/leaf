<script lang="ts">
  // The day viewer is an absolute-inset overlay, so it needs a positioned
  // frame. Cycles through a few fixture days with working prev/next/random.
  import DayViewer from '../lib/components/viewer/DayViewer.svelte';
  import { viewerDays } from './fixtures';

  let i = $state(viewerDays.length - 1);
  const day = $derived(viewerDays[i]);
</script>

<div class="frame">
  {#if day}
    <DayViewer
      {day}
      seriesName="Daily Sketch"
      hasPrev={i > 0}
      hasNext={i < viewerDays.length - 1}
      onPrev={() => (i -= 1)}
      onNext={() => (i += 1)}
      onRandom={() => (i = Math.floor(Math.random() * viewerDays.length))}
      onClose={() => undefined}
      onJump={() => undefined}
    />
  {/if}
</div>

<style>
  .frame {
    position: relative;
    width: 100%;
    height: 100%;
    min-height: 600px;
    background: var(--canvas);
    border-radius: var(--radius-lg);
    overflow: hidden;
  }
</style>
