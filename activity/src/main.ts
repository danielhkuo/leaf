import { mount } from 'svelte';

import App from './App.svelte';
import './app.css';

const target = document.getElementById('app');
if (!target) {
  throw new Error('#app mount point missing from index.html');
}

// The admin panel is a browser page at /admin — a separate lazy chunk so it
// costs gallery users nothing. Everything else is the in-Discord gallery.
if (location.pathname.startsWith('/admin')) {
  void import('./views/admin/Admin.svelte').then(({ default: Admin }) => {
    mount(Admin, { target });
  });
} else {
  mount(App, { target });
}
