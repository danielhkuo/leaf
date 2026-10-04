// The phone-width pass, as a suite: every mock screen at every width, with
// its usual text and with worst-case text (`&long=1`), measured for sideways
// overflow, tap-target size, field text size and accessibility. The console
// and the network are checked for every test by the `guard` fixture. (The
// minimised screens are measured at a tile's size instead: tile.spec.ts.)

import { axeViolations } from '../support/axe';
import { auditLayout } from '../support/layout';
import { expect, FULL_SCREENS, test } from '../support/mock';
import { VIEWPORTS, widthTag } from '../support/viewports';

for (const viewport of VIEWPORTS) {
  test.describe(`${viewport.width}px`, { tag: widthTag(viewport) }, () => {
    test.use(viewport.use);

    for (const { id } of FULL_SCREENS) {
      for (const longText of [false, true]) {
        test(`${id}${longText ? ', long text' : ''}`, async ({ page, mock }) => {
          await mock.open(id, { longText });
          await expect(page.getByText(/There is no mock screen called/)).toHaveCount(0);

          const layout = await auditLayout(page);
          expect.soft(layout.pageOverflow, 'the page scrolls sideways by this many px').toBe(0);
          expect.soft(layout.outside, 'elements past the edge of the screen').toEqual([]);
          expect.soft(layout.smallTargets, 'controls under the minimum tap target').toEqual([]);
          expect.soft(layout.smallFonts, 'fields iOS would zoom into').toEqual([]);
          expect.soft(await axeViolations(page), 'axe violations').toEqual([]);
        });
      }
    }
  });
}
