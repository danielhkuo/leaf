// Test support: a plain object made reactive. For a stand-in for a store
// whose changes a component has to react to, as it does to the real one's.

/** `value` as Svelte state: writes through the returned object are seen by effects. */
export function reactive<T extends object>(value: T): T {
  const state = $state(value);
  return state;
}
