import { mount, type Component } from 'svelte';

import App from './App.svelte';
import './app.css';
import { preloadSdk } from './lib/stores/session.svelte';

const found = document.getElementById('app');
if (!found) {
  throw new Error('#app mount point missing from index.html');
}
const target: HTMLElement = found;

/** Swaps index.html's static boot shell for a mounted view. */
function show(view: Component): void {
  target.replaceChildren();
  mount(view, { target });
}

/** A lazy view whose chunk did not arrive. These are browser pages: reload works. */
function showLoadFailure(e: unknown): void {
  console.error('leaf: a page chunk did not load', e);
  const note = document.createElement('p');
  note.className = 'boot-shell';
  note.setAttribute('role', 'alert');
  note.textContent = 'leaf didn’t finish loading. Check your connection and reload the page.';
  target.replaceChildren(note);
}

// Discord always launches an Activity with `frame_id`; without it this is a
// browser tab, where the SDK can only throw.
const inDiscord = new URLSearchParams(location.search).has('frame_id');

if (location.pathname.startsWith('/admin')) {
  // The admin panel is a browser page at /admin — a separate lazy chunk so it
  // costs gallery users nothing.
  void import('./views/admin/Admin.svelte').then(
    ({ default: Admin }) => show(Admin),
    showLoadFailure,
  );
} else if (inDiscord) {
  // Start the SDK chunk before the shell mounts, so it downloads while the
  // first frame renders.
  preloadSdk();
  show(App);
} else {
  // Someone opened the public URL directly. Explain where the gallery lives;
  // the SDK chunk is never fetched.
  void import('./views/Landing.svelte').then(
    ({ default: Landing }) => show(Landing),
    showLoadFailure,
  );
}
