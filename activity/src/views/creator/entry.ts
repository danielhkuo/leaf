// Lazy chunk entry for the creator views. Gallery dynamically imports this
// module on first navigation to a creator view, so none of this code ships in
// the initial gallery bundle (PERF.md budget).
//
// Named `entry` (not `index`) so the emitted chunk is `entry-<hash>.js` rather
// than `index-<hash>.js`, which would collide with the app's own entry chunk
// and confuse the bundle-size check's entry detection.

export { default as CreateSeries } from './CreateSeries.svelte';
export { default as MySeries } from './MySeries.svelte';
export { default as SeriesSettings } from './SeriesSettings.svelte';
