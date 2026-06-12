# leaf — Svelte 5 / TypeScript Code Quality Guidelines

Applies to everything under `activity/` (`**/*.svelte`, `**/*.ts`,
`**/*.svelte.ts`). You are writing senior-level, modern Svelte 5 — runes,
not legacy reactivity.

> **Reference docs (read before writing Svelte):** the complete Svelte
> documentation is vendored locally at
> [`docs/reference/svelte-llms-full.txt`](reference/svelte-llms-full.txt)
> (≈1.1MB, **local-only, gitignored**). If it's missing, restore it with:
> `curl -sL https://svelte.dev/llms-full.txt -o docs/reference/svelte-llms-full.txt`
> Consult it for any API you're not 100% sure of — especially runes
> semantics, snippets/`{@render}`, transitions, and attachment points where
> Svelte 5 differs from Svelte 4 idioms you may remember. Do not write
> Svelte 4 patterns from memory.

> **Adaptation note:** the leaf gallery is a **Vite SPA inside Discord's
> iframe**, not a SvelteKit app. SvelteKit-specific conventions (file-based
> `src/routes`, server route handlers, SSR, SEO/OpenGraph) do not apply:
> our backend is axum (`leaf-server`), our "routing" is in-app view state,
> and search engines can never see inside the Discord client. Equivalents
> are specified below; everything else from the standard Svelte 5 rules
> applies in full.

## Project structure

```
activity/
├── src/
│   ├── lib/
│   │   ├── components/   # by feature/domain: heatmap/, viewer/, picker/,
│   │   │                 # admin/, shared/
│   │   ├── stores/       # global reactive state (.svelte.ts modules)
│   │   ├── types/        # shared TS types (API DTOs, domain types)
│   │   ├── api/          # typed API client for leaf-server (+ zod schemas)
│   │   ├── sdk/          # embedded-app-sdk handshake & wrappers
│   │   └── utils/        # pure helpers (dates, day math, formatting)
│   ├── views/            # top-level screens (Gallery, DayViewer, Admin)
│   ├── App.svelte        # shell: view switching, theming, error boundary
│   └── main.ts
└── tests/                # Vitest setup; component tests co-located *.test.ts
```

- Components in `src/lib/components`, organized by feature/domain.
- Global state in `src/lib/stores`, shared types in `src/lib/types`,
  utilities in `src/lib/utils`.
- *(SvelteKit `src/routes` equivalent)*: top-level screens live in
  `src/views/`; navigation is explicit view state in the shell. No router
  library unless a real need is demonstrated (bundle discipline).
- *(API routes equivalent)*: there are no in-app API endpoints. All
  endpoints are axum in `leaf-server`. The frontend talks to them only
  through `src/lib/api`, which validates inputs/outputs with zod against
  the OpenAPI contract.

## TypeScript & style

- TypeScript everywhere, `strict: true`, plus
  `noUncheckedIndexedAccess: true` and `exactOptionalPropertyTypes: true`.
- No `any` (lint-enforced); `unknown` + narrowing at boundaries. API
  responses are parsed with zod, not blindly cast.
- PascalCase components, camelCase variables/functions, SCREAMING_SNAKE
  for true constants.
- Prettier (with `prettier-plugin-svelte`) and ESLint (flat config:
  `typescript-eslint` strict + `eslint-plugin-svelte`) are CI gates, as is
  `svelte-check`.

## Components & reactivity (runes — Svelte 5 only)

- One responsibility per component. If it scrolls past ~200 lines, split.
- `$state` for reactive variables: `let count = $state(0)`.
- `$derived` for computed values — never recompute in markup, never sync
  state with effects when a derivation works.
- `$effect` **only** for true side effects (SDK calls, observers, media
  element control), always returning cleanup when it allocates:

  ```svelte
  $effect(() => {
    const obs = new IntersectionObserver(onIntersect);
    obs.observe(node);
    return () => obs.disconnect();
  });
  ```

- Effect discipline is a performance rule here, not just style: careless
  `$effect`/store churn is how idle CPU stops being zero (see Performance).
- No Svelte 4 patterns: no `$:` reactive statements, no `export let`, no
  `createEventDispatcher`, no slot syntax in new code.

## Props, events, children

- `$props` with destructuring and defaults:
  `let { greeting = 'Hello!', onSelect }: Props = $props();` — typed via an
  explicit `Props` interface.
- Events are callback props: `<Day onOpen={(d) => open(d)} />`. No
  dispatched custom events.
- Content projection via the `children` snippet prop, not slots:

  ```svelte
  let { children } = $props();
  <div class="card">{@render children?.()}</div>
  ```

## State management

- Global state lives in `src/lib/stores` as **runes-based `.svelte.ts`
  modules** (universal reactivity) with typed interfaces — this is the
  Svelte 5 native form of the store pattern:

  ```ts
  // src/lib/stores/gallery.svelte.ts
  export const gallery = $state<GalleryState>({ seriesId: null, day: null });
  export const currentStreak = $derived(/* ... */);
  ```

- Classic `writable`/`derived` stores are acceptable where an external
  subscription contract is genuinely needed; prefer runes modules
  otherwise. Don't mix both patterns for the same piece of state.
- Server data is not global state: fetched via the API client, cached
  deliberately (and evicted — see RAM budget), never mirrored into a
  store "just in case".

## Testing & accessibility

- Vitest + Svelte Testing Library for components; plain Vitest for pure
  TS (`utils/`, `api/`, view-model logic). Pure logic is extracted from
  components precisely so it can be tested exhaustively.
- Tests assert **correctness only** — behavior, DOM state, a11y roles.
  Never timing, never animation frames, never pixels (timing/visual feel
  is human-verified per the phase guide).
- Mock at the API-client boundary, not `fetch`; mock the SDK module, not
  Discord.
- Semantic HTML first (`button`, `nav`, `figure`, `time`); ARIA only
  where semantics fall short (the heatmap grid gets `role="grid"` +
  labels). Every interactive element keyboard-reachable and operable;
  focus is managed on view changes (viewer open/close returns focus).
- Every image has descriptive alt text (the day caption; falls back to
  "Day N of {series}").
- `prefers-reduced-motion` honored: transitions collapse to instant.
- axe-core runs in component tests where wirable; violations fail.

## Styling

- Scoped `<style>` blocks. No CSS framework, no Tailwind, no CSS-in-JS.
- Design tokens as CSS custom properties in one file
  (`src/app.css`): colors (Discord light/dark via
  `prefers-color-scheme`), spacing scale, radii, motion durations.
  Components consume tokens; they do not invent values.
- Animate **only** `transform` and `opacity` (GPU-composited). Never
  animate layout properties (width/height/top/left/margin).

## Performance (first-class requirement — see PLAN.md)

The app runs in a sandboxed iframe inside an already-heavy client, on
phones. These are rules, not suggestions; Phase 19 measures them.

- **RAM budget**: thumbnails in grids; full-res only for the open day and
  **released on navigate-away** (clear `src`/revoke object URLs).
  Virtualize long lists — off-screen DOM does not exist.
- **Images**: `loading="lazy"`, `decoding="async"`, explicit
  width/height, `srcset` sized to device.
- **Idle = zero**: no polling, no perpetual timers/rAF loops, no effect
  churn at rest.
- **Bundle budget**: CI-enforced gzipped initial-chunk cap (50KB at
  scaffold; tightened in Phase 19). Non-MVP views (admin panel,
  evolution mode) are lazy `import()` chunks.
- **Dependency discipline**: every dependency is bundle + CSP + audit
  surface and is justified in the PR. Platform APIs first: View
  Transitions, CSS grid, `Intl` for date formatting (no date libraries),
  pointer events for swipe.
- Debounce scroll/resize handlers; prefer IntersectionObserver to scroll
  math.

## SEO → not applicable, replaced by embed-context polish

The app renders only inside Discord's client; there is nothing for a
crawler to index, so meta tags/OpenGraph/heading-hierarchy-for-SEO rules
are dropped. What replaces them:

- A proper `<title>` and theme-color for the iframe's own document.
- Heading hierarchy and semantic structure are still required — for
  accessibility, not SEO.
- The *bot's* link embeds (e.g. `/search` results) are leaf-server's
  concern, not the frontend's.

## CI gates (frontend, every PR)

1. `prettier --check`
2. `eslint` (flat config, max-warnings 0)
3. `svelte-check` (no errors)
4. `vitest run`
5. bundle-size budget on the built initial chunk
