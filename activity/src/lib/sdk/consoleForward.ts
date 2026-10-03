// Sends console output to the Discord client, whose Debug Logs are the only
// console a phone has. The SDK does this itself by default, but it drops the
// promise each forwarded line returns, so a line the client refuses becomes
// an unhandled rejection. App reports those and logs them, which forwards
// another line: a loop. This copy never lets a refused line escape.

/** The console methods forwarded, as the SDK names them. */
export const FORWARDED_LEVELS = ['log', 'warn', 'debug', 'info', 'error'] as const;

export type ConsoleLevel = (typeof FORWARDED_LEVELS)[number];

/** Sends one line to the client. It may reject, or throw. */
export type SendLine = (level: ConsoleLevel, message: string) => Promise<unknown>;

/**
 * Wraps `target`'s methods so each call is also passed to `send`. Output
 * still reaches the original method, and a line that cannot be sent is
 * dropped without a trace.
 */
export function forwardConsole(target: Pick<Console, ConsoleLevel>, send: SendLine): void {
  for (const level of FORWARDED_LEVELS) {
    const original = target[level];
    target[level] = (...args: unknown[]): void => {
      try {
        // The SDK's own format, so the client's log reads the same.
        send(level, args.join(' ')).catch(() => undefined);
      } catch {
        // Not even sendable (a Symbol argument, no SDK channel): drop it.
      }
      original.apply(target, args);
    };
  }
}
