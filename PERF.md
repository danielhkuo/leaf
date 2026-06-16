# leaf — performance & accessibility baseline (Phase 19)

A dedicated pass that treats the PLAN's performance callout and a11y as
exit-gated work. Every metric below has a **budget**; CI-measurable ones are
filled in, device-measured ones are marked **TODO** for the manual Android pass
(see *How to measure*). Re-run this against the recorded numbers whenever the
gallery changes.

## Bundle (CI-enforced)

| Chunk | Budget | Measured (gzipped) | Notes |
| --- | --- | --- | --- |
| Initial JS (`index-*.js`) | **40 KB** | **34.2 KB** | gate: `npm run bundle:check` |
| Initial CSS | — | ~3.0 KB | |
| Discord SDK (`discord-*.js`) | deferred | ~43 KB | lazy — loaded *after* first paint, not on the critical path |
| Admin panel (`Admin-*.js`) | deferred | ~4 KB | lazy — only loaded at `/admin`, zero cost to gallery users |

The budget was tightened from 50 KB → 40 KB to measured reality + headroom.

## Server (verified + tested)

- **Media proxy streams** from R2 (`Body::from_stream(get.into_stream())`) — no
  full-file buffering, so memory per request is bounded regardless of file
  size. Covered by `media_streams_the_original_with_immutable_cache_headers`.
- **R2 client reuse**: one `Arc<dyn ObjectStore>` is built once in the
  composition root and shared across every request (connection pooling lives in
  the object-store client).
- **SQLite concurrency**: `db::connect` sets WAL journal mode, foreign keys on,
  a 5 s busy timeout, and a pool of 8 — archive writes and gallery reads proceed
  concurrently without lock errors.
- **Immutable caching**: media responses carry
  `Cache-Control: public, max-age=31536000, immutable`, and signed-URL expiries
  are quantized to a daily bucket so Cloudflare's edge serves repeat views
  without touching R2 (one URL per attachment per day across all sessions).

## Idle = zero (verified by review)

- No `setInterval` / `setTimeout` loops and no `requestAnimationFrame` loops in
  the app — nothing runs at rest.
- Animations are CSS-only (skeleton shimmer, blur-up fade); they unmount with
  their elements and are disabled under `prefers-reduced-motion`.
- `IntersectionObserver`s (heatmap windowing) are event-driven and
  `disconnect()`ed on unmount.
- No polling: server data is fetched on demand; the per-series day index is
  cached, and full-res images are released on navigate-away.
- **TODO (device):** confirm idle CPU ≈ 0 with a 60 s performance trace.

## Accessibility

- **axe-core** runs in component tests (series picker, stats, day viewer) → **0
  violations** (`a11y.test.ts`; `color-contrast` excluded because jsdom has no
  layout — see device pass).
- **Keyboard**: archived-day cells and every control are real `button`/`a`
  elements (tab-reachable + operable). The day viewer handles ←/→/Esc, traps
  focus into the dialog, and restores it on close.
- **Alt text** = the day caption, falling back to "Day N of {series}".
- **Reduced motion**: transitions collapse to instant (`app.css` + `BlurImage`).
- **TODO (device):** full keyboard traverse and an axe DevTools pass in the real
  Discord client (desktop + Android), including color-contrast.

## Frontend runtime

The **structural invariants** behind the device metrics are unit-tested in CI —
they don't need a device. Only the **raw numbers** (paint ms, frames, heap MB on
the target hardware in Discord's webview) are inherently manual.

### Automated invariants (CI)

- **DOM stays bounded while scrolling** — off-screen heatmap blocks mount zero
  cells and don't even fetch until scrolled into view (`MonthBlock.test.ts`).
  This is the structural core of the Phase-16 "bounded DOM" exit criterion.
- **Memory flat across a browse** — exactly one full-res `<img>` survives day
  navigation (no accumulation) and it is released on close (`DayViewer.test.ts`).
- **Idle = zero** — no timers/polling/rAF (verified by review, above).

### Device numbers (manual — fill from the Android pass)

| Metric | Budget | Measured |
| --- | --- | --- |
| Cold load (launch → first paint), mid-range Android | < ~3 s | TODO |
| Heatmap scroll, 3-year synthetic series | no visible jank (≈ 60 fps) | TODO |
| JS heap after 50 open/close | returns to baseline | TODO |
| Idle CPU (60 s trace at rest) | ≈ 0 | TODO |

## How to measure (device pass)

1. Seed a multi-year synthetic series (≥ 3 years of days) for a real-scale test.
2. Launch the gallery in **real Discord on a mid-range Android**; attach Chrome
   devtools to the Android webview.
3. **Cold load**: record launch → first paint.
4. **Heatmap**: scroll the full archive; watch the Performance panel for jank and
   the Elements panel for bounded DOM node count.
5. **Day viewer**: open/close 50 days; confirm the JS heap returns to baseline
   (release-on-leave working).
6. **Idle**: leave the gallery at rest 60 s; confirm a flat CPU trace.
7. Record the numbers in the table above; each must be within its budget.

## Exit criteria

- [x] Bundle budget measured and CI-enforced (34.2 KB < 40 KB).
- [x] Server streaming, R2 reuse, and SQLite WAL/busy-timeout verified + tested.
- [x] axe clean on the tested components; reduced-motion and focus handled.
- [ ] Device numbers (cold load, scroll FPS, idle CPU, memory after 50-day
      browse) recorded and within budget — **the remaining manual step.**
