// In-app navigation as a small back-stack (no router library — bundle
// discipline). The shell renders `nav.current`; mobile back / swipe-down
// pops the stack so the viewer closes to Home rather than the app.

export type View =
  | { name: 'picker' }
  | { name: 'home'; seriesId: number }
  | { name: 'viewer'; seriesId: number; day: number }
  | { name: 'createSeries' }
  | { name: 'mySeries' }
  | { name: 'seriesSettings'; seriesId: number };

const PICKER: View = { name: 'picker' };

const stack = $state<View[]>([PICKER]);

export const nav = {
  /** The view on top of the stack. */
  get current(): View {
    return stack[stack.length - 1] ?? PICKER;
  },
  /** Whether there is somewhere to go back to. */
  get canGoBack(): boolean {
    return stack.length > 1;
  },
  /** Pushes a new view. */
  push(view: View): void {
    stack.push(view);
  },
  /** Replaces the entire stack with a single view. */
  reset(view: View): void {
    stack.splice(0, stack.length, view);
  },
  /** Pops back one view, if possible. */
  back(): void {
    if (stack.length > 1) stack.pop();
  },
};
