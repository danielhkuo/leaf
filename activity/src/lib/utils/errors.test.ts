import { describe, expect, it } from 'vitest';

import { ApiError, describeError, errorKind, isRetryable } from './errors';

describe('ApiError', () => {
  it.each([
    [401, 'unauthorized', false],
    [403, 'forbidden', false],
    [404, 'not_found', false],
    [400, 'rejected', false],
    [409, 'rejected', false],
    [422, 'rejected', false],
    [408, 'server', true],
    [429, 'server', true],
    [500, 'server', true],
    [503, 'server', true],
  ] as const)('derives kind and retryable from status %i', (status, kind, retryable) => {
    const e = new ApiError(status, 'GET /x');
    expect(e.kind).toBe(kind);
    expect(e.retryable).toBe(retryable);
  });

  it('lets the caller set the kind and the server override retryable', () => {
    expect(new ApiError(0, 'x', undefined, { kind: 'network' }).retryable).toBe(true);
    expect(new ApiError(0, 'x', undefined, { kind: 'timeout' }).retryable).toBe(true);
    expect(new ApiError(200, 'x', undefined, { kind: 'bad_response' }).retryable).toBe(false);
    expect(new ApiError(503, 'x', 'c', { retryable: false }).retryable).toBe(false);
  });

  it('keeps the positional constructor older callers use', () => {
    const e = new ApiError(409, 'POST /series → 409', 'name_taken');
    expect(e).toBeInstanceOf(Error);
    expect(e.name).toBe('ApiError');
    expect(e.code).toBe('name_taken');
    expect(e.detail).toBeUndefined();
  });
});

describe('describeError', () => {
  it('never shows the developer label', () => {
    const e = new ApiError(500, 'GET /guilds/123/series → 500', 'internal');
    expect(describeError(e)).not.toContain('GET');
    expect(describeError(e)).toBe('Something went wrong on leaf’s side. Try again in a moment.');
  });

  it('has a sentence for every kind', () => {
    const cases: [ApiError, RegExp][] = [
      [new ApiError(0, 'x', undefined, { kind: 'network' }), /connection/],
      [new ApiError(0, 'x', undefined, { kind: 'timeout' }), /too long/],
      [new ApiError(401, 'x'), /Close leaf and open it again/],
      [new ApiError(403, 'x'), /access/],
      [new ApiError(404, 'x'), /isn’t available/],
      [new ApiError(200, 'x', undefined, { kind: 'bad_response' }), /Close leaf and open it again/],
      [new ApiError(400, 'x'), /couldn’t do that/],
    ];
    for (const [e, pattern] of cases) expect(describeError(e)).toMatch(pattern);
  });

  it('prefers copy for a known server code', () => {
    expect(describeError(new ApiError(503, 'x', 'discord_unavailable'))).toBe(
      'leaf can’t reach Discord right now. Try again in a moment.',
    );
    expect(describeError(new ApiError(403, 'x', 'guild_not_setup'))).toContain('/setup');
  });

  it('uses the server’s sentence for a refusal it has no copy for', () => {
    const e = new ApiError(422, 'x', 'invalid_limit', { detail: 'The limit must be 1 to 25.' });
    expect(describeError(e)).toBe('The limit must be 1 to 25.');
    // A server sentence never replaces the copy for outages.
    const down = new ApiError(500, 'x', 'internal', { detail: 'db error' });
    expect(describeError(down)).not.toContain('db error');
  });

  it('handles values that are not ApiErrors', () => {
    expect(describeError(new TypeError('Failed to fetch'))).toBe(
      'Something went wrong. Try again.',
    );
    expect(describeError('boom')).toBe('Something went wrong. Try again.');
    expect(errorKind(new Error('x'))).toBe('unknown');
    expect(isRetryable(new Error('x'))).toBe(true);
    expect(isRetryable(new ApiError(404, 'x'))).toBe(false);
  });
});
