// What the owner's own list (`GET .../series/mine`) says about a series'
// channel. A current server sends no name, and `channel_missing`, once the
// channel was deleted or leaf lost sight of it. A server that sends only the
// missing name means the same, for a series that has a channel: one that
// never had any has none to lose. An older one may still send the name it
// last knew, with no flag: that reads as a channel that is there.

import type { MySeries } from '../types/api';

export interface SeriesChannel {
  /** Without `#`; `null` when there is none to show. */
  name: string | null;
  /** Deleted, or hidden from leaf: nothing posted there can be archived. */
  gone: boolean;
}

/**
 * A series' channel as its owner's list has it. The server's flag decides
 * when it is sent, either way; without it, a channel with no name is a
 * missing channel.
 */
export function channelOf(
  series: Pick<MySeries, 'channel_id' | 'channel_name' | 'channel_missing'>,
): SeriesChannel {
  const name = series.channel_name || null;
  const gone = series.channel_missing ?? (series.channel_id !== null && name === null);
  return { name: gone ? null : name, gone };
}
