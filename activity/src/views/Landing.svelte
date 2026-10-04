<script lang="ts">
  // What a browser tab gets at the public URL. The gallery only runs inside
  // Discord (the SDK needs Discord's launch parameters), so this says the
  // install is up, where the gallery opens, and where admins sign in.
</script>

<main class="landing">
  <div class="card">
    <span class="mark" aria-hidden="true">🍃</span>
    <h1>leaf is running</h1>
    <p class="lead">The gallery opens inside Discord, not in a browser tab.</p>

    <h2>Open the gallery</h2>
    <ul>
      <li>
        <strong>On a phone:</strong> in a server channel, tap <strong>+</strong> next to the message
        box, then <strong>Apps</strong>, then <strong>leaf</strong>.
      </li>
      <li>
        <strong>On desktop:</strong> click the <strong>Apps</strong> button in the message box, then
        <strong>leaf</strong>.
      </li>
    </ul>

    <h2>Server admins</h2>
    <p>Change leaf’s settings for your server and manage its series in the admin panel.</p>
    <a class="button" href="/admin">Open the admin panel</a>
  </div>
</main>

<style>
  .landing {
    display: grid;
    place-items: center;
    min-height: 100%;
    padding: var(--space-lg) var(--space-md);
  }
  .card {
    width: 100%;
    max-width: 32rem;
    padding: var(--space-lg);
    background: var(--surface-1);
    border-radius: var(--radius-xl);
    box-shadow: var(--shadow-card);
  }
  .mark {
    font-size: var(--fs-display);
    line-height: 1;
  }
  h1 {
    margin: var(--space-xs) 0 0;
    font-size: var(--fs-headline);
    font-weight: var(--fw-display);
    line-height: 1.15;
    letter-spacing: var(--tracking-tight);
  }
  .lead {
    margin: var(--space-xs) 0 0;
    color: var(--ink-muted);
  }
  h2 {
    margin: var(--space-lg) 0 var(--space-xs);
    font-size: var(--fs-subhead);
    font-weight: var(--fw-display);
    line-height: 1.2;
  }
  ul {
    display: grid;
    gap: var(--space-xs);
    margin: 0;
    padding-left: 1.25em;
  }
  p {
    margin: 0;
  }
  .button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-height: var(--control-height);
    margin-top: var(--space-md);
    padding: 0 26px;
    color: var(--inverse-ink);
    font-weight: var(--fw-display);
    text-decoration: none;
    background: var(--inverse-canvas);
    border-radius: var(--radius-pill);
    box-shadow: var(--shadow-soft);
  }
  @media (hover: hover) {
    .button:hover {
      filter: brightness(1.04);
    }
  }
</style>
