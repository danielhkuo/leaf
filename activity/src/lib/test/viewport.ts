// Test support: jsdom's window as a browser's is when Discord resizes the
// Activity. jsdom lays nothing out, so the size is just two numbers and the
// event that says they changed.

/** jsdom's own window size. */
export const FULL_SIZE = { width: 1024, height: 768 } as const;
/** The tile Discord for Android shrinks a minimised Activity to. */
export const TILE_SIZE = { width: 120, height: 120 } as const;

/** Sizes the window and says so, as a browser does. */
export function resizeTo(size: { width: number; height: number }): void {
  Object.assign(window, { innerWidth: size.width, innerHeight: size.height });
  window.dispatchEvent(new Event('resize'));
}
