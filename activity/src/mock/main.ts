// Entry for the mock screen viewer (dev-only; reached at /mock.html). Mounts a
// gallery of every screen with fixture data — no Discord SDK, network, or auth.
import { mount } from 'svelte';

import '../app.css';
import ScreenViewer from './ScreenViewer.svelte';

const target = document.getElementById('app');
if (!target) {
  throw new Error('#app mount point missing from mock.html');
}

mount(ScreenViewer, { target });
