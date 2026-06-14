# leaf activity (embedded app)

The gallery: a Vite + Svelte 5 SPA that runs inside Discord's activity iframe
and talks to `leaf-server` over the REST API. See
`docs/svelte-guidelines.md` for code standards and the UI/UX plan for the
screen design.

## Scripts

| Command                           | What it does                                            |
| --------------------------------- | ------------------------------------------------------- |
| `npm run dev`                     | Vite dev server on :5173, proxying `/api` → leaf-server |
| `npm run build`                   | `svelte-check` then `vite build` → `dist/`              |
| `npm run check`                   | Type-check (`svelte-check`)                             |
| `npm run lint`                    | ESLint (flat config; strict)                            |
| `npm run format` / `format:check` | Prettier                                                |
| `npm run test` / `test:run`       | Vitest (watch / once)                                   |
| `npm run bundle:check`            | Fail if the initial JS chunk blows the budget           |

## Local dev loop (real Discord)

1. **Run leaf-server** (the backend + API) on `:8080`:
   ```sh
   cargo run --bin leaf
   ```
2. **Configure the app**: `cp .env.example .env` and set
   `VITE_DISCORD_CLIENT_ID` to your dev application's client id.
3. **Run the dev server**: `npm install` then `npm run dev`. Vite serves the
   app on `:5173` and proxies `/api` to leaf-server, so the app is
   same-origin (as it is in production behind Discord's proxy).
4. **Expose it** with a quick tunnel so Discord can reach it:
   ```sh
   cloudflared tunnel --url http://localhost:5173
   ```
5. **Point a dev Discord application** at the tunnel: in the dev portal, enable
   Activities and set the URL mapping `/` → the tunnel hostname (full Discord
   app wiring is Phase 18). Launch the activity from a server you've added the
   app to.

> The SDK handshake only completes **inside** the Discord client; opening
> `localhost:5173` in a plain browser will reach the "Connecting to Discord…"
> state and then error, which is expected.

## Production

`vite build` emits `dist/`, which `leaf-server` serves from `STATIC_DIR`
(the Docker image builds the frontend and sets `STATIC_DIR=/app/dist`). App
and API are one origin, behind one Discord URL mapping.
