<script lang="ts">
  // The gallery and creator screens are the real thing: views/Gallery.svelte
  // with its store and API client, a made-up session in place of the Discord
  // handshake, and `fetch` answered from the fixtures (api.ts). So what the
  // mock shows cannot fall behind what Discord shows, and every tap works.
  import { onDestroy, untrack } from 'svelte';

  import { nav, type View } from '../lib/stores/nav.svelte';
  import { session as sessionStore } from '../lib/stores/session.svelte';
  import Gallery from '../views/Gallery.svelte';
  import { installMockApi, type Scenario } from './api';
  import { mockSession } from './fixtures';

  interface Props {
    /** What the mock server holds; the fixtures as they are when omitted. */
    scenario?: Scenario | undefined;
    /** The views to open on, bottom of the back-stack first. */
    stack: [View, ...View[]];
  }
  let { scenario, stack }: Props = $props();

  // Before Gallery mounts: the API client keeps the `fetch` it finds when the
  // store builds it. One scenario per page (ScreenViewer gives each screen
  // its own iframe), so it is read once.
  onDestroy(installMockApi(untrack(() => scenario)));

  // The archive steps are worded for the client leaf runs in.
  const session = mockSession(window.innerWidth < 768 ? 'mobile' : 'desktop');
  // The create form reads the launch channel from the session store.
  sessionStore.value = { status: 'authed', session };

  // Gallery chooses its own first view once the series list is in (the list,
  // or the series last opened here). This screen's views then replace it. A
  // scenario whose list never loads stays on Gallery's own error screen.
  const before = nav.version;
  let placed = false;
  $effect(() => {
    if (placed || nav.version === before) return;
    placed = true;
    untrack(() => nav.reset(...stack));
  });
</script>

<Gallery {session} />
