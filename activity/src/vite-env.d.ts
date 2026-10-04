/// <reference types="svelte" />
/// <reference types="vite/client" />

interface ImportMetaEnv {
  /**
   * Discord application (client) id. Optional: inside Discord the id is read
   * from the `<id>.discordsays.com` hostname, which always wins. This only
   * stands in when the page is served from another host.
   */
  readonly VITE_DISCORD_CLIENT_ID?: string;
  /** Optional API base override; defaults to same-origin `/api`. */
  readonly VITE_API_BASE?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
