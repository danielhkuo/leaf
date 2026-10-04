// The widths every mock screen is checked at. The phone ones are what the
// hand-run "phone-width pass" used: the narrowest iPhone and Android layouts,
// the common iPhone width and a large phone. Each is a real device's
// descriptor (touch, mobile user agent, its pixel ratio), so media queries
// such as `(hover: hover)` answer as they do on that device.

import { devices, type PlaywrightTestOptions } from '@playwright/test';

type Emulation = Partial<
  Pick<
    PlaywrightTestOptions,
    'viewport' | 'userAgent' | 'deviceScaleFactor' | 'isMobile' | 'hasTouch'
  >
>;

export interface Viewport {
  /** The CSS width, as the test titles show it. */
  width: number;
  /** A touch phone. WebKit, the nearest CI stand-in for iOS, runs these. */
  phone: boolean;
  use: Emulation;
}

/**
 * A device from Playwright's registry, without the engine it names: the
 * project decides the engine, and a describe block cannot change it.
 */
function device(name: keyof typeof devices): Emulation {
  const found = devices[name];
  if (!found) throw new Error(`Playwright has no device called ${name}`);
  const { viewport, userAgent, deviceScaleFactor, isMobile, hasTouch } = found;
  return { viewport, userAgent, deviceScaleFactor, isMobile, hasTouch };
}

export const VIEWPORTS: readonly Viewport[] = [
  { width: 320, phone: true, use: device('iPhone SE') },
  { width: 360, phone: true, use: device('Galaxy S8') },
  { width: 375, phone: true, use: device('iPhone 13 Mini') },
  { width: 430, phone: true, use: device('iPhone 14 Pro Max') },
  { width: 768, phone: false, use: device('iPad Mini') },
  // A desktop window: no touch, and the engine's own user agent.
  { width: 1280, phone: false, use: { viewport: { width: 1280, height: 800 } } },
];

/** The viewport of that width. */
export function viewport(width: number): Viewport {
  const found = VIEWPORTS.find((v) => v.width === width);
  if (!found) throw new Error(`no viewport is ${width}px wide`);
  return found;
}

/** Tags a describe block, so a project can leave the wide viewports out. */
export const PHONE_TAG = '@phone';
export const WIDE_TAG = '@wide';
export function widthTag(v: Viewport): string {
  return v.phone ? PHONE_TAG : WIDE_TAG;
}
