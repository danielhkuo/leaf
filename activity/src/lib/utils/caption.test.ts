import { describe, expect, it } from 'vitest';

import { formatCaption } from './caption';

describe('formatCaption', () => {
  it('leaves ordinary text and its line breaks alone', () => {
    expect(formatCaption('  Day 5!\r\nSecond line\n\nThird <3  ')).toBe(
      'Day 5!\nSecond line\n\nThird <3',
    );
  });

  it('names custom emoji instead of showing their ids', () => {
    expect(formatCaption('done <:leaf:123456789012345678> <a:party_blob:987654321>')).toBe(
      'done :leaf: :party_blob:',
    );
  });

  it('writes Discord timestamps as dates in the given zone', () => {
    // 2023-11-14T22:13:20Z is already the 15th in Tokyo.
    const options = { locale: 'en-US', timeZone: 'Asia/Tokyo' };
    expect(formatCaption('posted <t:1700000000:R>', options)).toBe('posted Nov 15, 2023');
    expect(formatCaption('<t:1700000000>', { locale: 'en-US', timeZone: 'UTC' })).toBe(
      'Nov 14, 2023',
    );
  });

  it('replaces mentions it has no names for with plain words', () => {
    expect(formatCaption('with <@123456> and <@!123456>, for <@&99> in <#4242>')).toBe(
      'with @member and @member, for @role in #channel',
    );
  });

  it('shows a slash command by name and unwraps a link', () => {
    expect(formatCaption('use </wrapped:1234> see <https://example.com/a?b=1>')).toBe(
      'use /wrapped see https://example.com/a?b=1',
    );
  });

  it('does not touch text that only looks similar', () => {
    const text = 'a < b > c, <b>bold</b>, <t:soon>, <@everyone>, 1 <:> 2';
    expect(formatCaption(text)).toBe(text);
  });
});
