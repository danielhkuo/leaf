// Captions are stored as the Discord message's raw text. Chat renders its
// markup; here it would show as `<:leaf:1234>` or `<t:1700000000:R>`. This
// turns the pieces a reader can't parse into plain words. It produces text,
// never HTML: the caption is other people's content.

import { formatPostedAt } from './datetime';

export interface CaptionOptions {
  locale?: string | undefined;
  /** The zone dates are shown in (the server's), as on the calendar. */
  timeZone?: string | undefined;
}

// <:name:id> and <a:name:id>; <t:unix> and <t:unix:style>; <@id>, <@!id>,
// <@&id>; <#id>; </command name:id>; and a link wrapped to hide its embed.
const MARKUP =
  /<a?:(\w{1,64}):\d{1,25}>|<t:(-?\d{1,13})(?::[tTdDfFR])?>|<@&\d{1,25}>|<@!?\d{1,25}>|<#\d{1,25}>|<\/([^:>\n]{1,100}):\d{1,25}>|<(https?:\/\/[^\s>]+)>/g;

/** A caption as readable text: Discord's own markup replaced by words. */
export function formatCaption(raw: string, options: CaptionOptions = {}): string {
  return raw
    .replace(
      MARKUP,
      (
        whole: string,
        emoji: string | undefined,
        unix: string | undefined,
        command: string | undefined,
        link: string | undefined,
      ) => {
        if (emoji !== undefined) return `:${emoji}:`;
        if (unix !== undefined)
          return formatPostedAt(Number(unix), options.locale, options.timeZone);
        if (command !== undefined) return `/${command}`;
        if (link !== undefined) return link;
        if (whole.startsWith('<@&')) return '@role';
        if (whole.startsWith('<@')) return '@member';
        return '#channel';
      },
    )
    .replace(/\r\n?/g, '\n')
    .trim();
}
