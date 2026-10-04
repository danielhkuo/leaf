import { render } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';

import ArchiveSteps from './ArchiveSteps.svelte';

/** The steps as a person reads them: one line each, however the markup wraps. */
function steps(container: HTMLElement): string[] {
  return [...container.querySelectorAll('li')].map((li) =>
    (li.textContent ?? '').replace(/\s+/g, ' ').trim(),
  );
}

describe('ArchiveSteps', () => {
  it('on a phone: one tap to a step, with the app under the name Discord lists it by', () => {
    const { container } = render(ArchiveSteps, {
      props: { platform: 'mobile', channelName: 'daily-sketch', appName: 'Sketchbook Archive' },
    });

    expect(steps(container)).toEqual([
      'Minimise leaf, then post your photo or video in #daily-sketch.',
      'Press and hold your message.',
      'Tap Apps. Scroll down in the menu to find it.',
      'Choose Sketchbook Archive, then Archive to Series.',
      'Check the day number leaf suggests and confirm it. The day shows up here.',
    ]);
    // What to look for in Discord's menus stands out from the sentence around it.
    const named = [...container.querySelectorAll('strong')].map((el) => el.textContent);
    expect(named).toEqual(['#daily-sketch', 'Apps', 'Sketchbook Archive', 'Archive to Series']);
  });

  it('calls the app leaf when Discord gave no name', () => {
    const { container } = render(ArchiveSteps, {
      props: { platform: 'mobile', channelName: 'daily-sketch' },
    });

    expect(steps(container)).toContain('Choose leaf, then Archive to Series.');
  });

  it('names no channel when the series’ own is not known: any series channel will do', () => {
    const phone = render(ArchiveSteps, { props: { platform: 'mobile', channelName: null } });
    expect(steps(phone.container)[0]).toBe(
      'Minimise leaf, then post your photo or video in one of this server’s series channels.',
    );
    expect(phone.container.textContent).not.toContain('#');
    phone.unmount();

    const desktop = render(ArchiveSteps, { props: { platform: 'desktop', channelName: null } });
    expect(steps(desktop.container)[0]).toBe(
      'Post your photo or video in one of this server’s series channels.',
    );
  });

  it('on desktop: right-click, Apps, Archive to Series, with no list of apps in between', () => {
    const { container } = render(ArchiveSteps, {
      props: { platform: 'desktop', channelName: 'daily-sketch', appName: 'Sketchbook Archive' },
    });

    expect(steps(container)).toEqual([
      'Post your photo or video in #daily-sketch.',
      'Right-click your message, choose Apps, then Archive to Series.',
      'Check the day number leaf suggests and confirm it. The day shows up here.',
    ]);
  });
});
