import { render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';

import { expectNoA11yViolations } from '../../test/a11y';
import Skeleton from './Skeleton.svelte';

describe('Skeleton', () => {
  it('is a status whose label is real text, not only an attribute', () => {
    render(Skeleton);
    const status = screen.getByRole('status');
    expect(status).toHaveTextContent('Loading');
    expect(status).not.toHaveAttribute('aria-busy');
    expect(status).not.toHaveAttribute('aria-label');
  });

  it('says what is loading when told', () => {
    render(Skeleton, { props: { label: 'Loading calendar' } });
    expect(screen.getByRole('status')).toHaveTextContent('Loading calendar');
  });

  it('is decorative with an empty label, so a screen of blocks speaks once', () => {
    const { container } = render(Skeleton, { props: { label: '' } });
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    expect(container.querySelector('.skeleton')).toHaveAttribute('aria-hidden', 'true');
  });

  it('has no axe violations, labelled or decorative', async () => {
    const labelled = render(Skeleton);
    await expectNoA11yViolations(labelled.container);
    const decorative = render(Skeleton, { props: { label: '' } });
    await expectNoA11yViolations(decorative.container);
  });
});
