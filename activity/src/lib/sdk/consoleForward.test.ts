import { describe, expect, it, vi } from 'vitest';

import { forwardConsole, type ConsoleLevel } from './consoleForward';

function fakeConsole(): Record<ConsoleLevel, ReturnType<typeof vi.fn>> {
  return { log: vi.fn(), warn: vi.fn(), debug: vi.fn(), info: vi.fn(), error: vi.fn() };
}

describe('forwardConsole', () => {
  it('sends each line to the client and still writes it locally', () => {
    const target = fakeConsole();
    const { error, info } = target;
    const send = vi.fn().mockResolvedValue(null);

    forwardConsole(target, send);
    target.error('leaf: boot stopped', 'network');
    target.info('ready');

    expect(send).toHaveBeenCalledWith('error', 'leaf: boot stopped network');
    expect(send).toHaveBeenCalledWith('info', 'ready');
    expect(error).toHaveBeenCalledWith('leaf: boot stopped', 'network');
    expect(info).toHaveBeenCalledWith('ready');
  });

  it('drops a line the client refuses, without an unhandled rejection', async () => {
    const target = fakeConsole();
    const { warn } = target;
    const onUnhandled = vi.fn();
    process.on('unhandledRejection', onUnhandled);
    try {
      forwardConsole(target, () => Promise.reject(new Error('CAPTURE_LOG refused')));
      target.warn('slow');
      // Unhandled rejections are reported after the microtask queue drains.
      await new Promise((resolve) => setTimeout(resolve, 0));

      expect(onUnhandled).not.toHaveBeenCalled();
      expect(warn).toHaveBeenCalledWith('slow');
    } finally {
      process.off('unhandledRejection', onUnhandled);
    }
  });

  it('still writes locally when the line cannot be sent at all', () => {
    const target = fakeConsole();
    const { log } = target;
    const symbol = Symbol('id');

    forwardConsole(target, () => {
      throw new Error('Attempting to send message before initialization');
    });
    target.log(symbol);

    expect(log).toHaveBeenCalledWith(symbol);
  });
});
