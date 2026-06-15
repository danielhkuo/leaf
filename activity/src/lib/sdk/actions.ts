// Post-handshake SDK commands, each lazily importing the SDK module so it
// stays in the deferred chunk (see discord.ts and the bundle budget). The
// dynamic import resolves instantly after boot — the chunk is already loaded.

/** Opens a link (e.g. a jump-to-message URL) through the Discord client. */
export async function openExternalLink(url: string): Promise<void> {
  const { getSdk } = await import('./discord');
  await getSdk().commands.openExternalLink({ url });
}
