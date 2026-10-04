// What keeps an app test on this machine, tried in every engine the app
// projects run.
//
// The guard cuts off a request to another origin with a route (guard.ts; its
// mechanics are tested in e2e/mock/checks.spec.ts). A redirect is another
// matter: the browser follows it by itself, past every route handler, and
// that is how leaf's own /admin/login reaches discord.com. So the app
// projects also send every address but 127.0.0.1 to a proxy that is not
// there (`LOOPBACK_ONLY` in playwright.config.ts). Here the server redirects
// to another machine, as /admin/login does, and the browser has to get
// nowhere.

import { expect, test } from '../support/app';

/**
 * How each engine words a connection that was refused, which is all the
 * missing proxy can give. A browser with no proxy would say something else:
 * that it looked the name up and did not find it (Chromium's
 * `ERR_NAME_NOT_RESOLVED`; WebKit on macOS, "A server with the specified
 * hostname could not be found."), and to say that it has asked the network.
 * WebKit takes its words from the platform, and only macOS's are known here.
 * It has two for the same refusal: "Could not connect to the server." on a
 * quiet machine, and "The network connection was lost." on a busy one (about
 * one run in five while the mock suites run beside this one). So its pattern
 * takes any that name a connection or a proxy, and none that name a lookup.
 */
const REFUSED: Partial<Record<string, RegExp>> = {
  chromium: /^net::ERR_PROXY_CONNECTION_FAILED$/,
  webkit: /could not connect|connection refused|connection was lost|proxy/i,
};

test('a redirect to another machine goes nowhere, though no route can see it', async ({
  page,
  guard,
  request,
  browserName,
}) => {
  const refused = REFUSED[browserName];
  if (!refused) throw new Error(`how ${browserName} words a refused connection is not known`);

  // The server's answer, unfollowed: a redirect like /admin/login's, to a
  // name that never resolves. Even a browser that followed it would arrive
  // nowhere, so this test cannot itself be what leaves the machine.
  const answer = await request.get('/__e2e/elsewhere', { maxRedirects: 0 });
  expect(answer.status()).toBe(303);
  const elsewhere = answer.headers().location;
  expect(elsewhere).toBe('http://leaf-e2e.invalid/landed');

  const failure = await page.goto('/__e2e/elsewhere').then(
    () => null,
    (error: Error) => error.message,
  );
  expect(failure, 'the navigation has to fail').not.toBeNull();
  expect(page.url()).toBe('about:blank');

  // The guard saw the browser set off, and how it was stopped: at the proxy,
  // before the name was looked up.
  const [left, failed, ...rest] = guard.unexpected();
  expect(left).toBe(`request left the server under test: GET ${elsewhere}`);
  const stopped = /^request failed: GET http:\/\/leaf-e2e\.invalid\/landed \((.*)\)$/.exec(
    failed ?? '',
  );
  expect(stopped?.[1], failed).toMatch(refused);
  // WebKit says it once more, in the console.
  for (const line of rest) {
    const said = /^console error: Failed to load resource: (.*)$/.exec(line);
    expect(said?.[1], line).toMatch(refused);
  }
  guard.allow(/leaf-e2e\.invalid/, /^console error: Failed to load resource: /);
});
