<script lang="ts">
  // The series' reaction emoji: a few common ones to tap, and a small box for
  // any other (typed from the emoji keyboard). The bot reacts with the whole
  // value, so it has to be exactly one standard emoji; the form checks that
  // on save. No `maxlength`: it counts UTF-16 units and would cut a family
  // or flag emoji in half.
  interface Props {
    id: string;
    value: string;
    /** The problem with the current value, if the last save found one. */
    error: string | null;
    onedit: () => void;
  }
  let { id, value = $bindable(), error, onedit }: Props = $props();

  /** Six, so they stay on one row of 44px targets at phone width. */
  const SUGGESTED = ['🍃', '📷', '🎨', '✏️', '☕', '⭐'];

  function pick(emoji: string): void {
    value = emoji;
    onedit();
  }
</script>

<div class="field">
  <label class="field-label" for={id}>Reaction emoji</label>
  <div class="row">
    <input
      {id}
      class="control"
      bind:value
      autocomplete="off"
      autocapitalize="off"
      spellcheck="false"
      aria-invalid={error ? 'true' : undefined}
      aria-describedby="{id}-hint{error ? ` ${id}-error` : ''}"
      oninput={onedit}
    />
    <div class="chips" role="group" aria-label="Common choices">
      {#each SUGGESTED as emoji (emoji)}
        <button
          type="button"
          class="chip"
          aria-pressed={value.trim() === emoji}
          onclick={() => pick(emoji)}
        >
          {emoji}
        </button>
      {/each}
    </div>
  </div>
  <p class="hint" id="{id}-hint">
    Shown beside the series, and the reaction leaf adds to each post it archives. Tap one, or type
    any single emoji.
  </p>
  {#if error}<p class="field-error" id="{id}-error">{error}</p>{/if}
</div>

<style>
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-xs);
    align-items: center;
  }
  .control {
    flex: none;
    width: 4.5rem;
    font-size: 1.25rem;
    text-align: center;
  }
  .control[aria-invalid='true'] {
    border-color: var(--error-fill);
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-xxs);
    min-width: 0;
  }
  .chip {
    width: var(--touch-target);
    height: var(--touch-target);
    padding: 0;
    font: inherit;
    font-size: 1.25rem;
    line-height: 1;
    background: var(--surface-2);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-pill);
    cursor: pointer;
  }
  .chip[aria-pressed='true'] {
    background: var(--surface-3);
    border-color: var(--border-strong);
  }
  .chip:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .hint,
  .field-error {
    margin: 0;
    font-size: var(--fs-caption);
  }
  .hint {
    color: var(--ink-muted);
  }
  .field-error {
    color: var(--error);
    font-size: var(--fs-body-sm);
  }
  @media (hover: hover) {
    .chip:hover:not(:disabled) {
      background: var(--surface-3);
    }
  }
</style>
