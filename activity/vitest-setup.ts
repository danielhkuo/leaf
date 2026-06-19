import '@testing-library/jest-dom/vitest';

// jsdom has no ResizeObserver; Svelte's `bind:clientWidth/Height` needs one.
// A no-op stub is enough — tests don't assert on measured sizes.
if (!('ResizeObserver' in globalThis)) {
  globalThis.ResizeObserver = class {
    observe = (): void => undefined;
    unobserve = (): void => undefined;
    disconnect = (): void => undefined;
  } as unknown as typeof ResizeObserver;
}
