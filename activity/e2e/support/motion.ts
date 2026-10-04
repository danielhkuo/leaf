// What still moves when the device asks for reduced motion. app.css cuts
// every animation to a single 0.01ms run there, with two deliberate
// exceptions that override it: the loading spinner (Spinner.svelte) and the
// boot mark (App.svelte) stop turning and "breathe" instead, so a loading
// screen still reads as working rather than frozen. That is opacity only,
// and slow. Anything else that never ends is a failure.

import type { Page } from '@playwright/test';

/** The slowest a never-ending fade may cycle: faster than this is a flicker. */
const MIN_BREATH_MS = 1_000;

/**
 * Markup with an endless animation as a component might add one, with no
 * thought for reduced motion. With motion allowed `endlessMotion` reports
 * it; under reduced motion app.css's reset must cut it to a single run.
 */
export const RESTLESS =
  '<style>@keyframes nudge { to { transform: translateX(4px); } }</style>' +
  '<div class="restless" style="animation: nudge 1s infinite">Restless</div>';

/** One line per animation that runs forever and is not a slow, opacity-only fade. */
export function endlessMotion(page: Page): Promise<string[]> {
  return page.evaluate((minBreathMs) => {
    const IGNORED = new Set(['offset', 'computedOffset', 'easing', 'composite']);
    return document.getAnimations().flatMap((animation) => {
      const effect = animation.effect;
      if (!(effect instanceof KeyframeEffect)) return [];
      const timing = effect.getComputedTiming();
      if (timing.iterations !== Infinity) return [];
      const animated = new Set(
        effect.getKeyframes().flatMap((frame) => Object.keys(frame).filter((k) => !IGNORED.has(k))),
      );
      const duration = Number(timing.duration);
      const breathing = animated.size === 1 && animated.has('opacity') && duration >= minBreathMs;
      if (breathing) return [];
      const name = animation instanceof CSSAnimation ? animation.animationName : 'an animation';
      const target = effect.target;
      const where = target
        ? `${target.tagName.toLowerCase()}.${[...target.classList].filter((c) => !c.startsWith('svelte-')).join('.')}${effect.pseudoElement ?? ''}`
        : 'a removed element';
      return [`${name} on ${where}: ${[...animated].join(', ')} every ${duration}ms, forever`];
    });
  }, MIN_BREATH_MS);
}
