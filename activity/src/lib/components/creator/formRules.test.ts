import { describe, expect, it } from 'vitest';

import {
  channelLabel,
  charCount,
  daysAre,
  descriptionProblem,
  fieldForCode,
  isSingleEmoji,
  nameProblem,
  parseDayNumber,
} from './formRules';

describe('charCount', () => {
  it('counts code points, as the server does', () => {
    expect(charCount('abc')).toBe(3);
    expect(charCount('🍃🍃')).toBe(2);
    expect('🍃🍃'.length).toBe(4);
  });
});

describe('nameProblem', () => {
  it('accepts 2 to 40 characters after trimming', () => {
    expect(nameProblem('  ab  ')).toBeNull();
    expect(nameProblem('x'.repeat(40))).toBeNull();
    // Forty emoji are forty characters, though eighty UTF-16 units.
    expect(nameProblem('🍃'.repeat(40))).toBeNull();
  });

  it('says a name is missing before it complains about length', () => {
    expect(nameProblem('   ')).toBe('Give the series a name.');
    expect(nameProblem('a')).toMatch(/2 to 40 characters/);
    expect(nameProblem('x'.repeat(41))).toMatch(/2 to 40 characters/);
  });

  it('refuses what Discord would turn into a ping', () => {
    for (const name of ['hi @Everyone', '@here now', 'for <@123>', 'in <#456>', '<@&789> only']) {
      expect(nameProblem(name)).toMatch(/mention/);
    }
    expect(nameProblem('me @ home')).toBeNull();
  });

  it('refuses control characters', () => {
    expect(nameProblem('two\tparts')).toMatch(/one line/);
    expect(nameProblem('two\nlines')).toMatch(/one line/);
  });
});

describe('descriptionProblem', () => {
  it('allows 200 characters of trimmed text', () => {
    expect(descriptionProblem('')).toBeNull();
    expect(descriptionProblem(` ${'x'.repeat(200)} `)).toBeNull();
    expect(descriptionProblem('x'.repeat(201))).toMatch(/200 characters/);
  });
});

describe('parseDayNumber', () => {
  it('reads whole numbers from 1 to 999999', () => {
    expect(parseDayNumber('1')).toBe(1);
    expect(parseDayNumber(' 200 ')).toBe(200);
    expect(parseDayNumber('007')).toBe(7);
    expect(parseDayNumber('999999')).toBe(999_999);
    expect(parseDayNumber('４２')).toBe(42);
  });

  it('refuses everything else', () => {
    for (const text of [
      '',
      ' ',
      '0',
      '-3',
      '1.5',
      '1e3',
      '1,000',
      'Day 4',
      '1000000',
      '12345678',
    ]) {
      expect(parseDayNumber(text)).toBeNull();
    }
  });
});

describe('isSingleEmoji', () => {
  it('accepts one standard emoji in any of its shapes', () => {
    const one = [
      '🍃',
      ' 🍃 ',
      '✏️', // pictograph + emoji presentation
      '☕',
      '👍🏽', // skin tone
      '👩‍🚀', // joined sequence
      '👨‍👩‍👧‍👦', // family
      '🇯🇵', // flag
      '🏴󠁧󠁢󠁳󠁣󠁴󠁿', // subdivision flag
      '1️⃣', // keycap
      '#⃣',
      '©',
    ];
    for (const emoji of one) expect(isSingleEmoji(emoji), emoji).toBe(true);
  });

  it('refuses text, several emoji and custom emoji', () => {
    const not = [
      '',
      '  ',
      'a',
      'abc',
      'é',
      '🍃🍃',
      '🍃 🍃',
      '<:leaf:123>',
      '1',
      '🇯',
      '🍃a',
      '🏽',
      '👩‍',
    ];
    for (const value of not) expect(isSingleEmoji(value), value).toBe(false);
  });
});

describe('channelLabel', () => {
  it('names a channel, or says leaf cannot see it', () => {
    expect(channelLabel({ id: '1', name: 'art' })).toBe('#art');
    expect(channelLabel({ id: '123456789', name: null })).toBe('A channel leaf can’t see (…6789)');
  });
});

describe('daysAre', () => {
  it('agrees with the number', () => {
    expect(daysAre(1)).toBe('1 day is');
    expect(daysAre(3)).toBe('3 days are');
  });
});

describe('fieldForCode', () => {
  it('matches server codes to the field they are about', () => {
    expect(fieldForCode('name_taken')).toBe('name');
    expect(fieldForCode('invalid_name')).toBe('name');
    expect(fieldForCode('invalid_description')).toBe('description');
    expect(fieldForCode('invalid_emoji')).toBe('emoji');
    expect(fieldForCode('missing_privacy_role')).toBe('role');
    expect(fieldForCode('unknown_role')).toBe('role');
    expect(fieldForCode('invalid_channel')).toBe('channel');
    expect(fieldForCode('invalid_start_day')).toBe('startDay');
    expect(fieldForCode('reminder_time_required')).toBe('reminderTime');
    expect(fieldForCode('invalid_timezone')).toBe('reminderTz');
  });

  it('places the settings form’s own codes for a save that was not applied', () => {
    expect(fieldForCode('name_not_saved')).toBe('name');
    expect(fieldForCode('start_day_not_saved')).toBe('startDay');
  });

  it('leaves policy, unknown and missing codes to the form as a whole', () => {
    expect(fieldForCode('max_series')).toBeNull();
    expect(fieldForCode('constructor')).toBeNull();
    expect(fieldForCode(undefined)).toBeNull();
    expect(fieldForCode(null)).toBeNull();
  });
});
