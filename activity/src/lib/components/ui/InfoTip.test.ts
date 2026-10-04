import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';

import { expectNoA11yViolations } from '../../test/a11y';
import InfoTip, { bubbleShift } from './InfoTip.svelte';

describe('bubbleShift', () => {
  it('leaves a bubble that already fits where it is', () => {
    expect(bubbleShift(200, 240, 375)).toBe(0);
  });

  it('slides right when the bubble would cross the left edge', () => {
    // Centred on 100 a 240px bubble starts at -20; the gutter is 8.
    expect(bubbleShift(100, 240, 375)).toBe(28);
  });

  it('slides left when the bubble would cross the right edge', () => {
    // Centred on 340 it would end at 460; it must end at 375 - 8.
    expect(bubbleShift(340, 240, 375)).toBe(-93);
  });

  it('pins a bubble wider than the viewport to the left gutter', () => {
    expect(bubbleShift(150, 400, 320)).toBe(58);
  });

  it('honours a custom gutter', () => {
    expect(bubbleShift(100, 240, 375, 16)).toBe(36);
  });
});

describe('InfoTip', () => {
  const TEXT = 'Time zone for daily reminders.';

  function renderTip() {
    const view = render(InfoTip, { props: { text: TEXT, label: 'Timezone' } });
    return { ...view, dot: screen.getByRole('button', { name: 'Help: Timezone' }) };
  }

  it('starts closed and opens on click', async () => {
    const { dot } = renderTip();
    expect(dot).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByText(TEXT)).not.toBeInTheDocument();

    await fireEvent.click(dot);

    expect(dot).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByText(TEXT)).toBeInTheDocument();
  });

  it('puts the explanation in the live region the button controls', async () => {
    const { dot } = renderTip();
    await fireEvent.click(dot);

    const region = screen.getByRole('status');
    expect(dot).toHaveAttribute('aria-controls', region.id);
    expect(region).toHaveTextContent(TEXT);
  });

  it('closes on a second click', async () => {
    const { dot } = renderTip();
    await fireEvent.click(dot);
    await fireEvent.click(dot);
    expect(screen.queryByText(TEXT)).not.toBeInTheDocument();
  });

  it('closes on Escape and returns focus to the button', async () => {
    const { dot } = renderTip();
    await fireEvent.click(dot);
    await fireEvent.keyDown(document.body, { key: 'Escape' });

    expect(screen.queryByText(TEXT)).not.toBeInTheDocument();
    expect(dot).toHaveFocus();
  });

  it('closes when the pointer goes down outside it', async () => {
    const { dot } = renderTip();
    await fireEvent.click(dot);
    await fireEvent.pointerDown(document.body);
    expect(screen.queryByText(TEXT)).not.toBeInTheDocument();
  });

  it('stays open when the pointer goes down on its own button', async () => {
    const { dot } = renderTip();
    await fireEvent.click(dot);
    await fireEvent.pointerDown(dot);
    expect(screen.getByText(TEXT)).toBeInTheDocument();
  });

  it('closes when its bubble is clicked', async () => {
    const { dot } = renderTip();
    await fireEvent.click(dot);
    await fireEvent.click(screen.getByText(TEXT));

    expect(screen.queryByText(TEXT)).not.toBeInTheDocument();
    expect(dot).toHaveAttribute('aria-expanded', 'false');
  });

  it('keeps focus on the button while its bubble is pressed', async () => {
    const { dot } = renderTip();
    await fireEvent.click(dot);

    // A cancelled mousedown moves no focus, so focusout cannot remove the
    // bubble halfway through a tap and let the rest of it fall through.
    const notCancelled = await fireEvent.mouseDown(screen.getByText(TEXT));

    expect(notCancelled).toBe(false);
    expect(screen.getByText(TEXT)).toBeInTheDocument();
  });

  it('keeps a click on the bubble away from a surrounding label and its handlers', async () => {
    const label = document.createElement('label');
    const checkbox = document.createElement('input');
    checkbox.type = 'checkbox';
    label.append(checkbox);
    document.body.append(label);
    const onLabelClick = vi.fn();
    label.addEventListener('click', onLabelClick);

    try {
      render(InfoTip, { target: label, props: { text: TEXT, label: 'Sprout probation' } });
      await fireEvent.click(screen.getByRole('button', { name: 'Help: Sprout probation' }));
      onLabelClick.mockClear();

      await fireEvent.click(screen.getByText(TEXT));

      expect(checkbox).not.toBeChecked();
      expect(onLabelClick).not.toHaveBeenCalled();
      expect(screen.queryByText(TEXT)).not.toBeInTheDocument();
    } finally {
      label.remove();
    }
  });

  it('has no axe violations, closed or open', async () => {
    const { container, dot } = renderTip();
    await expectNoA11yViolations(container);
    await fireEvent.click(dot);
    await expectNoA11yViolations(container);
  });
});
