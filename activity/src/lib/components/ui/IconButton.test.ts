import { fireEvent, render, screen } from '@testing-library/svelte';
import { createRawSnippet } from 'svelte';
import { describe, expect, it, vi } from 'vitest';

import { expectNoA11yViolations } from '../../test/a11y';
import IconButton from './IconButton.svelte';

describe('IconButton', () => {
  it('draws a named icon as a decorative SVG and keeps the label on the button', () => {
    render(IconButton, { props: { ariaLabel: 'Back to series list', icon: 'back' } });

    const button = screen.getByRole('button', { name: 'Back to series list' });
    const svg = button.querySelector('svg');
    expect(svg).toHaveAttribute('aria-hidden', 'true');
    expect(button).toHaveTextContent('');
  });

  it.each(['back', 'close', 'gear', 'plus'] as const)('has a drawing for "%s"', (icon) => {
    render(IconButton, { props: { ariaLabel: icon, icon } });
    const shapes = screen.getByRole('button', { name: icon }).querySelectorAll('path, circle');
    expect(shapes.length).toBeGreaterThan(0);
  });

  it('never submits a surrounding form', () => {
    render(IconButton, { props: { ariaLabel: 'Close', icon: 'close' } });
    expect(screen.getByRole('button', { name: 'Close' })).toHaveAttribute('type', 'button');
  });

  it('reports clicks', async () => {
    const onclick = vi.fn();
    render(IconButton, { props: { ariaLabel: 'Start a series', icon: 'plus', onclick } });
    await fireEvent.click(screen.getByRole('button', { name: 'Start a series' }));
    expect(onclick).toHaveBeenCalledOnce();
  });

  it('still renders caller-supplied glyphs when no icon is named', () => {
    const children = createRawSnippet(() => ({ render: () => '<span>‹</span>' }));
    render(IconButton, { props: { ariaLabel: 'Previous day', disabled: true, children } });
    const button = screen.getByRole('button', { name: 'Previous day' });
    expect(button).toBeDisabled();
    expect(button).toHaveTextContent('‹');
    expect(button.querySelector('svg')).toBeNull();
  });

  it('draws the named icon instead of children when both are given', () => {
    const children = createRawSnippet(() => ({ render: () => '<span>✕</span>' }));
    render(IconButton, { props: { ariaLabel: 'Close', icon: 'close', children } });
    const button = screen.getByRole('button', { name: 'Close' });
    expect(button.querySelector('svg')).not.toBeNull();
    expect(button).toHaveTextContent('');
  });

  it('has no axe violations', async () => {
    const { container } = render(IconButton, {
      props: { ariaLabel: 'Series settings', icon: 'gear' },
    });
    await expectNoA11yViolations(container);
  });
});
