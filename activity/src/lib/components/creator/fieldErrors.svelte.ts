// Which error a form shows under which field. Two sources feed it: the
// form's own checks on submit, and the server's answer to the last submit,
// which the parent passes down as a sentence plus the server's code. Either
// kind is shown under the field it is about, and is dropped as soon as that
// field is edited, so a message never outlives the value it was about.

import { SvelteMap } from 'svelte/reactivity';

/** The server's refusal of the last submit, already matched to a field. */
export interface ServerFailure<F extends string> {
  /** The field it belongs under, or `null` for the form as a whole. */
  field: F | null;
  message: string;
}

/**
 * Moves focus to a control and brings it, and the message under it, into
 * view. The forms keep their action bar stuck to the bottom edge, and a
 * browser's own focus scrolling counts a control behind that bar as already
 * on screen, so this scrolls by hand: the control to the middle, then on by
 * as little as shows its message (`<control id>-error`, which can sit a few
 * lines down, after the hint). The forms' `scroll-margin` keeps both clear
 * of the bar.
 */
export function revealField(control: HTMLElement): void {
  control.focus({ preventScroll: true });
  // Not every DOM has it (the test one does not).
  if (typeof control.scrollIntoView !== 'function') return;
  control.scrollIntoView({ block: 'center' });
  if (control.id === '') return;
  document.getElementById(`${control.id}-error`)?.scrollIntoView({ block: 'nearest' });
  // On a very short screen the message must not cost the control itself.
  if (control.getBoundingClientRect().top < 0) control.scrollIntoView({ block: 'start' });
}

export class FieldErrors<F extends string> {
  readonly #server: () => ServerFailure<F> | null;
  readonly #local = new SvelteMap<F, string>();
  /** Counts submits, so a dismissed server error stays dismissed only for its own submit. */
  #submit = $state(0);
  #dismissed = $state(-1);

  /** `server` reads the parent's props, so it is called again whenever they change. */
  constructor(server: () => ServerFailure<F> | null) {
    this.#server = server;
  }

  get #failure(): ServerFailure<F> | null {
    return this.#dismissed === this.#submit ? null : this.#server();
  }

  /** What the form's own checks found on a submit that was held back. */
  refuse(problems: Partial<Record<F, string>>): void {
    this.#local.clear();
    for (const [field, message] of Object.entries<string | undefined>(problems)) {
      if (message !== undefined) this.#local.set(field as F, message);
    }
  }

  /** The form's values went to the server: its next answer is a new one. */
  sent(): void {
    this.#local.clear();
    this.#submit += 1;
  }

  /** The message for a field, or `null`. */
  of(field: F): string | null {
    const failure = this.#failure;
    return this.#local.get(field) ?? (failure?.field === field ? failure.message : null);
  }

  /** The server failure that belongs to no field (network, policy, unknown). */
  get form(): string | null {
    const failure = this.#failure;
    return failure && failure.field === null ? failure.message : null;
  }

  /** The field was edited: its message no longer applies. */
  edited(field: F): void {
    this.#local.delete(field);
    if (this.#server()?.field === field) this.#dismissed = this.#submit;
  }

  /** Forgets everything (the form was reset). */
  clear(): void {
    this.#local.clear();
    this.#dismissed = this.#submit;
  }

  /** The first of `order` that has a message: the field to move focus to. */
  first(order: readonly F[]): F | null {
    return order.find((field) => this.of(field) !== null) ?? null;
  }
}
