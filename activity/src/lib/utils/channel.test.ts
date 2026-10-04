import { describe, expect, it } from 'vitest';

import { mySeriesSchema } from '../api/schemas';
import { channelOf, type SeriesChannel } from './channel';

/** A row of the owner's list as a server sends it, read as the client reads it. */
function read(channel: Record<string, unknown>): SeriesChannel {
  return channelOf(
    mySeriesSchema.parse({
      id: 7,
      name: 'Daily Sketch',
      emoji: '✏️',
      state: 'active',
      cadence: 'daily',
      channel_id: '900000000000000101',
      archived_days: 124,
      reminder_enabled: false,
      ...channel,
    }),
  );
}

const GONE: SeriesChannel = { name: null, gone: true };

describe('channelOf', () => {
  it('names a channel leaf can see', () => {
    const there: SeriesChannel = { name: 'daily-sketch', gone: false };
    expect(read({ channel_name: 'daily-sketch', channel_missing: false })).toEqual(there);
    // A server without the flag, or one that sends it empty.
    expect(read({ channel_name: 'daily-sketch' })).toEqual(there);
    expect(read({ channel_name: 'daily-sketch', channel_missing: null })).toEqual(there);
  });

  it('calls a channel gone when the server says so, with or without a name', () => {
    expect(read({ channel_name: null, channel_missing: true })).toEqual(GONE);
    expect(read({ channel_missing: true })).toEqual(GONE);
    // A name the server still had is not shown for a channel it calls gone.
    expect(read({ channel_name: 'general', channel_missing: true })).toEqual(GONE);
  });

  it('takes a missing name for a missing channel when the server sends no flag', () => {
    expect(read({ channel_name: null })).toEqual(GONE);
    expect(read({})).toEqual(GONE);
    expect(read({ channel_name: null, channel_missing: null })).toEqual(GONE);
  });

  it('does not call a channel gone for a series that never had one', () => {
    const none: SeriesChannel = { name: null, gone: false };
    // A server without the flag: no id, so no channel to have lost.
    expect(read({ channel_id: null, channel_name: null })).toEqual(none);
    expect(read({ channel_id: null })).toEqual(none);
    // A current server says the same with its flag.
    expect(read({ channel_id: null, channel_name: null, channel_missing: false })).toEqual(none);
  });

  it('does not call a channel gone when the server says it is not, name or no name', () => {
    // The name could not be had this time (Discord did not answer, say).
    expect(read({ channel_name: null, channel_missing: false })).toEqual({
      name: null,
      gone: false,
    });
  });
});
