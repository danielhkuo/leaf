// A controllable IntersectionObserver stub for tests (jsdom has none).
// Install it, render, then call `.enter()` to simulate scrolling a block
// into view — letting us assert the heatmap's windowing invariant.

export class MockIntersectionObserver {
  static instances: MockIntersectionObserver[] = [];
  readonly #cb: IntersectionObserverCallback;
  readonly #observed = new Set<Element>();

  constructor(cb: IntersectionObserverCallback) {
    this.#cb = cb;
    MockIntersectionObserver.instances.push(this);
  }

  observe(el: Element): void {
    this.#observed.add(el);
  }
  unobserve(el: Element): void {
    this.#observed.delete(el);
  }
  disconnect(): void {
    this.#observed.clear();
  }
  takeRecords(): IntersectionObserverEntry[] {
    return [];
  }

  /** Fire an intersection for everything this observer is watching. */
  enter(): void {
    const entries = [...this.#observed].map(
      (target) => ({ target, isIntersecting: true }) as IntersectionObserverEntry,
    );
    this.#cb(entries, this as unknown as IntersectionObserver);
  }
}

/** Replaces the global with a fresh mock; returns a restore function. */
export function installMockIO(): () => void {
  const real = globalThis.IntersectionObserver as typeof IntersectionObserver | undefined;
  MockIntersectionObserver.instances = [];
  globalThis.IntersectionObserver =
    MockIntersectionObserver as unknown as typeof IntersectionObserver;
  return () => {
    globalThis.IntersectionObserver = real as typeof IntersectionObserver;
  };
}
