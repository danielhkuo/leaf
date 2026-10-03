import { render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';

import { expectNoA11yViolations } from '../lib/test/a11y';
import Landing from './Landing.svelte';

describe('Landing', () => {
  it('says the install is up and where the gallery opens', () => {
    render(Landing);

    expect(screen.getByRole('heading', { level: 1, name: 'leaf is running' })).toBeInTheDocument();
    expect(screen.getByText(/opens inside Discord/)).toBeInTheDocument();
    expect(screen.getByText('On a phone:')).toBeInTheDocument();
    expect(screen.getByText('On desktop:')).toBeInTheDocument();
  });

  it('points admins at the admin panel and offers nothing to retry', () => {
    render(Landing);

    expect(screen.getByRole('link', { name: 'Open the admin panel' })).toHaveAttribute(
      'href',
      '/admin',
    );
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
  });

  it('has no axe violations', async () => {
    const { container } = render(Landing);
    await expectNoA11yViolations(container);
  });
});
