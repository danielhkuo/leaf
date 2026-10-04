// The measurements in e2e/support are only worth something if they can
// fail. Each test here breaks a healthy screen the way the measurement is
// meant to catch, and expects it to say so. A measurement that goes blind
// (a selector that matches nothing, a rule that is switched off) fails here
// rather than passing everywhere in silence.

import { axeViolations } from '../support/axe';
import { MOCK_PORT } from '../support/env';
import { auditLayout, CALENDAR_DAY } from '../support/layout';
import { expect, test } from '../support/mock';
import { endlessMotion, RESTLESS } from '../support/motion';
import { viewport } from '../support/viewports';

/**
 * A file the mock server has, for the tests that ask another origin for it.
 * A script: a browser drops a page sent across origins before it is read.
 */
const SCRIPT = '/src/mock/main.ts';

/** Adds markup to the end of the screen's main landmark. */
const APPEND = (html: string): void => {
  document.querySelector('main')?.insertAdjacentHTML('beforeend', html);
};

test.describe('at 375px', () => {
  test.use(viewport(375).use);

  test('a healthy screen measures clean, and has controls and fields to measure', async ({
    page,
    mock,
  }) => {
    await mock.open('settings');
    const layout = await auditLayout(page);
    expect(layout).toMatchObject({
      pageOverflow: 0,
      outside: [],
      smallTargets: [],
      smallFonts: [],
    });
    expect(layout.targets).toBeGreaterThan(10);
    expect(layout.fields).toBeGreaterThan(5);
  });

  test('overflow: an element wider than the screen is reported', async ({ page, mock }) => {
    await mock.open('picker');
    await page.evaluate(APPEND, '<div class="too-wide" style="width: 500px">wide</div>');
    const layout = await auditLayout(page);
    expect(layout.pageOverflow).toBeGreaterThan(0);
    expect(layout.outside.join('\n')).toContain('div.too-wide');
  });

  test('overflow: content a scrolling container holds is not', async ({ page, mock }) => {
    await mock.open('picker');
    await page.evaluate(
      APPEND,
      '<div style="overflow-x: auto"><div class="scrolled" style="width: 500px">wide</div></div>',
    );
    const layout = await auditLayout(page);
    expect(layout.pageOverflow).toBe(0);
    expect(layout.outside).toEqual([]);
  });

  test('overflow: content a clipping container cuts off is reported, as cut off', async ({
    page,
    mock,
  }) => {
    await mock.open('picker');
    await page.evaluate(
      APPEND,
      '<div class="clipper" style="overflow: hidden"><div class="cut" style="width: 500px">wide</div></div>',
    );
    const layout = await auditLayout(page);
    // It cannot widen the page, and it is still past the edge of the screen.
    expect(layout.pageOverflow).toBe(0);
    expect(layout.outside).toHaveLength(1);
    expect(layout.outside[0]).toContain('div.cut "wide" spans');
    expect(layout.outside[0]).toContain('cut off by div.clipper');
  });

  test('tap targets: a small button is reported, an inline link in a sentence is not', async ({
    page,
    mock,
  }) => {
    await mock.open('picker');
    await page.evaluate(
      APPEND,
      '<button class="tiny" style="width: 30px; height: 30px; padding: 0">x</button>' +
        '<p>Read <a class="in-sentence" href="#top">the guide</a> first.</p>' +
        '<a class="alone" href="#top" style="display: inline-block">x</a>',
    );
    const small = (await auditLayout(page)).smallTargets.join('\n');
    expect(small).toContain('button.tiny "x" is 30.0 x 30.0');
    expect(small).toContain('a.alone');
    expect(small).not.toContain('a.in-sentence');
  });

  test('tap targets: a checkbox counts with its label, and is reported without one', async ({
    page,
    mock,
  }) => {
    await mock.open('picker');
    await page.evaluate(
      APPEND,
      '<label style="display: flex; align-items: center; min-height: 44px">' +
        '<input class="labelled" type="checkbox" /> <span>Remind me, every day</span></label>' +
        '<input class="bare" type="checkbox" aria-label="Bare" />',
    );
    const small = (await auditLayout(page)).smallTargets.join('\n');
    expect(small).toContain('input.bare');
    expect(small).not.toContain('input.labelled');
  });

  test('tap targets: a control inside a closed disclosure is measured too', async ({
    page,
    mock,
  }) => {
    await mock.open('picker');
    await page.evaluate(
      APPEND,
      '<details><summary style="display: block; min-height: 44px">More</summary>' +
        '<button class="tucked-away" style="width: 30px; height: 30px; padding: 0">x</button>' +
        '</details>',
    );
    expect((await auditLayout(page)).smallTargets.join('\n')).toContain('button.tucked-away');
  });

  test('tap targets: a calendar day under 44px is reported at this width', async ({
    page,
    mock,
  }) => {
    await mock.open('home');
    await page
      .locator(CALENDAR_DAY)
      .first()
      .evaluate((cell: HTMLElement) => (cell.style.width = '42px'));
    const small = (await auditLayout(page)).smallTargets;
    expect(small).toHaveLength(1);
    expect(small[0]).toContain('is 42.0 x');
    expect(small[0]).toContain('(minimum 44px)');
  });

  test('field text: an input under 16px is reported', async ({ page, mock }) => {
    await mock.open('create');
    await page.evaluate(
      APPEND,
      '<input class="small-text" aria-label="Small" style="font-size: 15px" />',
    );
    expect((await auditLayout(page)).smallFonts).toEqual([
      'input.small-text "Small" has 15px text (minimum 16px)',
    ]);
  });

  test('axe: low contrast, which jsdom cannot see, is reported', async ({ page, mock }) => {
    await mock.open('picker');
    await page.evaluate(APPEND, '<p id="faint" style="color: #b9b4aa">Hard to read</p>');
    const violations = await axeViolations(page);
    expect(violations).toHaveLength(1);
    expect(violations[0]).toMatch(/^color-contrast: /);
    expect(violations[0]).toContain('#faint');
  });

  test('axe: text in a select is judged, with the chevron left as it was', async ({
    page,
    mock,
  }) => {
    await mock.open('settings');
    const select = page.getByRole('combobox').first();
    const chevron = (): Promise<string> =>
      select.evaluate((el) => getComputedStyle(el).backgroundImage);
    const drawn = await chevron();
    expect(drawn).toContain('gradient');

    await select.evaluate((el: HTMLElement) => (el.style.color = '#b9b4aa'));
    const violations = await axeViolations(page);
    expect(violations).toHaveLength(1);
    expect(violations[0]).toMatch(/^color-contrast: /);
    expect(violations[0]).toContain(`#${await select.getAttribute('id')}`);
    // Taken off for the scan only.
    expect(await chevron()).toBe(drawn);
    expect(await select.getAttribute('style')).toMatch(/^color: [^;]+;$/);
  });

  test('axe: what it cannot judge is reported, not passed over', async ({ page, mock }) => {
    await mock.open('picker');
    await page.evaluate(
      APPEND,
      '<p id="on-gradient" style="background: linear-gradient(#ffffff, #eeeeee)">On a gradient</p>' +
        // Not a chevron in the padding: a gradient behind the text itself.
        '<select id="striped" class="control" aria-label="Striped" style="background-size: 100% 100%">' +
        '<option>One</option></select>',
    );
    const findings = await axeViolations(page);
    expect(findings).toHaveLength(1);
    expect(findings[0]).toMatch(/^color-contrast, not judged: /);
    expect(findings[0]).toContain('#on-gradient (');
    expect(findings[0]).toContain('#striped (');
    expect(findings[0]).toContain('Reason: bgGradient');
  });

  test('axe: the calendar’s exception is for its labels, not for all text under a scrim', async ({
    page,
    mock,
  }) => {
    await mock.open('home');
    // The labels on the days' pictures are not judged, and are let through.
    await expect(page.locator(`${CALENDAR_DAY} span`).first()).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);

    // The same arrangement anywhere else is reported.
    await page.evaluate(
      APPEND,
      '<style>.veiled { position: relative; } .veiled::before { content: ""; position: absolute; ' +
        'inset: 0; background: rgb(0 0 0 / 50%); }</style>' +
        '<p class="veiled"><span id="veiled">Under a scrim</span></p>',
    );
    const findings = await axeViolations(page);
    expect(findings).toHaveLength(1);
    expect(findings[0]).toMatch(/^color-contrast, not judged: /);
    expect(findings[0]).toContain('#veiled (');
    expect(findings[0]).toContain('Reason: pseudoContent');
  });

  test('axe: a page with no main landmark is reported', async ({ page, mock }) => {
    await mock.open('picker');
    await page.evaluate(() => {
      const main = document.querySelector('main');
      main?.replaceWith(...main.childNodes);
    });
    expect((await axeViolations(page)).join('\n')).toContain('landmark-one-main');
  });

  test('console and network: errors, warnings and stray requests are reported', async ({
    page,
    mock,
    guard,
  }) => {
    await mock.open('picker');
    await page.route('**/refused', (route) => route.fulfill({ status: 500, body: '' }));
    await page.route('**/cut-off', (route) => route.abort());
    await page.evaluate(async () => {
      console.error('deliberate error');
      console.warn('deliberate warning');
      const requests = [
        // Not `fetch`, which the mock answers inside the page: this leaves it.
        '/api/guilds/1/nothing-answers-this',
        '/refused',
        '/cut-off',
        // This same machine under its other name: another origin to a browser.
        `http://localhost:${location.port}/elsewhere`,
      ];
      for (const url of requests) {
        await new Promise((done) => {
          const request = new XMLHttpRequest();
          request.addEventListener('loadend', done);
          request.open('GET', url);
          request.send();
        });
      }
    });
    const reported = [
      'console error: deliberate error',
      'console warning: deliberate warning',
      /API request reached the network: GET \S+\/nothing-answers-this/,
      /HTTP 500: GET \S+\/refused/,
      /request failed: GET \S+\/cut-off/,
      /request left the server under test: GET http:\/\/localhost:\d+\/elsewhere/,
    ];
    // The browser reports a request's end a moment after the page sees it.
    for (const line of reported) {
      await expect.poll(() => guard.unexpected().join('\n')).toMatch(line);
    }
    // Deliberate, all of it: the guard must not fail this test for them. The
    // browser logs each failed load to the console as well.
    guard.allow(
      /deliberate (error|warning)/,
      /nothing-answers-this|refused|cut-off|elsewhere/,
      /^console error:/,
    );
  });

  test('pictures: one the server has no file for is reported, by its answer and on the page', async ({
    page,
    mock,
    guard,
  }) => {
    await mock.open('picker');
    await page.evaluate(APPEND, '<img class="lost" alt="" src="/assets/not-there.png" />');
    // The dev server answers with the app's page and a 200.
    await expect(page.locator('img.lost')).toHaveJSProperty('complete', true);
    await expect
      .poll(() => guard.unexpected().join('\n'))
      .toMatch(/a page answered for a missing image: GET \S+\/assets\/not-there\.png/);
    await guard.inspect();
    expect(guard.unexpected().join('\n')).toMatch(
      /broken image: img\.lost \S+\/assets\/not-there\.png/,
    );
    guard.allow(/not-there\.png/);
  });

  test('network: a request or a socket to another origin never arrives', async ({
    page,
    mock,
    guard,
  }) => {
    await mock.open('picker');
    // This same machine under its other name: another origin to a browser,
    // and one that would answer. `no-cors`, so that an answer would count.
    const answered = await page.evaluate(
      (path) =>
        fetch(`http://localhost:${location.port}${path}`, { mode: 'no-cors' }).then(
          () => true,
          () => false,
        ),
      SCRIPT,
    );
    expect(answered, 'the request was answered').toBe(false);
    expect(guard.unexpected().join('\n')).toMatch(
      /request left the server under test: GET http:\/\/localhost:\d+\//,
    );

    // A socket is closed from this side, with the guard's own code.
    const closed = await page.evaluate(
      (path) =>
        new Promise<number>((done) => {
          const socket = new WebSocket(`ws://localhost:${location.port}${path}`);
          socket.addEventListener('close', (event) => done(event.code));
        }),
      SCRIPT,
    );
    expect(closed).toBe(1008);
    expect(guard.unexpected().join('\n')).toMatch(
      /socket left the server under test: ws:\/\/localhost:\d+\//,
    );
    guard.allow(/localhost/, /^console error:/);
  });

  test('motion: with motion allowed, the boot mark’s sway is seen', async ({ page, mock }) => {
    await mock.open('loading');
    const moving = await endlessMotion(page);
    expect(moving).toHaveLength(1);
    expect(moving[0]).toContain('transform');
  });

  test('motion: an endless animation is seen wherever it is', async ({ page, mock }) => {
    await mock.open('picker');
    await page.evaluate(APPEND, RESTLESS);
    expect(await endlessMotion(page)).toEqual([
      'nudge on div.restless: transform every 1000ms, forever',
    ]);
  });

  test('motion: a skeleton’s shimmer is seen while a screen loads', async ({ page, mock }) => {
    await mock.openLoading('picker');
    await expect(page.getByRole('status')).toHaveText(/^Loading/);
    const moving = await endlessMotion(page);
    expect(moving.length).toBeGreaterThan(0);
    // Svelte scopes the keyframes' name: `svelte-<hash>-shimmer`.
    for (const line of moving) {
      expect(line).toMatch(/shimmer on div\.skeleton::after: transform every 1400ms, forever$/);
    }
  });
});

test.describe('with a second origin named as the project’s own', () => {
  test.use({ ...viewport(375).use, alsoReachable: [`http://localhost:${MOCK_PORT}`] });

  test('network: a request to it arrives, and is not reported', async ({ page, mock, guard }) => {
    await mock.open('picker');
    const answered = await page.evaluate(
      (path) =>
        fetch(`http://localhost:${location.port}${path}`, { mode: 'no-cors' })
          // Read to the end: an answer left unread is dropped, and counts as failed.
          .then((response) => response.arrayBuffer())
          .then(
            () => true,
            () => false,
          ),
      SCRIPT,
    );
    expect(answered).toBe(true);
    expect(guard.unexpected()).toEqual([]);
  });
});

test.describe('at 320px', () => {
  test.use(viewport(320).use);

  test('tap targets: calendar days pass at 41.7px, the documented exception, and no smaller', async ({
    page,
    mock,
  }) => {
    await mock.open('home');
    const cell = page.locator(CALENDAR_DAY).first();
    const box = await cell.boundingBox();
    expect(box?.width).toBeGreaterThan(41);
    expect(box?.width).toBeLessThan(44);
    expect((await auditLayout(page)).smallTargets).toEqual([]);

    await cell.evaluate((el: HTMLElement) => (el.style.width = '36px'));
    const small = (await auditLayout(page)).smallTargets.join('\n');
    expect(small).toContain('the exception for calendar day cells below 345px');
  });
});
