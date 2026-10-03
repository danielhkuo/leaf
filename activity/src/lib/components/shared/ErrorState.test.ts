import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';

import { expectNoA11yViolations } from '../../test/a11y';
import ErrorState from './ErrorState.svelte';

describe('ErrorState', () => {
  it('announces the title and message as an alert', () => {
    render(ErrorState, {
      props: { title: 'Couldn’t load the calendar', message: 'Check your connection.' },
    });
    const alert = screen.getByRole('alert');
    expect(alert).toHaveTextContent('Couldn’t load the calendar');
    expect(alert).toHaveTextContent('Check your connection.');
  });

  it('shows no buttons when there is nothing to do', () => {
    render(ErrorState, { props: { title: 'Open leaf inside a server' } });
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
  });

  it('retries in place', async () => {
    const onRetry = vi.fn();
    render(ErrorState, { props: { title: 'Couldn’t load this day', onRetry } });

    await fireEvent.click(screen.getByRole('button', { name: 'Try again' }));

    expect(onRetry).toHaveBeenCalledOnce();
    expect(screen.queryByRole('button', { name: 'Back' })).not.toBeInTheDocument();
  });

  it('offers a way out next to Retry, with caller-chosen labels', async () => {
    const onRetry = vi.fn();
    const onBack = vi.fn();
    render(ErrorState, {
      props: { title: 'Couldn’t load this day', onRetry, onBack, backLabel: 'Close' },
    });

    await fireEvent.click(screen.getByRole('button', { name: 'Close' }));

    expect(onBack).toHaveBeenCalledOnce();
    expect(onRetry).not.toHaveBeenCalled();
  });

  it('has no axe violations', async () => {
    const { container } = render(ErrorState, {
      props: {
        title: 'Couldn’t load the gallery',
        message: 'leaf didn’t answer. Check your connection and try again.',
        onRetry: vi.fn(),
        onBack: vi.fn(),
      },
    });
    await expectNoA11yViolations(container);
  });
});
