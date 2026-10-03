// The slice of `@discord/embedded-app-sdk` our handshake depends on.
//
// Declaring it ourselves (rather than importing the SDK's types) keeps the
// handshake logic pure and mockable: tests construct a fake `SdkLike`
// instead of standing up the real SDK, which only works inside Discord.

/** A Discord user as returned by the SDK's `authenticate` command. */
export interface DiscordUser {
  id: string;
  username: string;
  /** The user's chosen display name; absent for legacy accounts. */
  global_name?: string | null;
}

/** Result of `commands.authorize` — the OAuth code we exchange server-side. */
export interface AuthorizeResult {
  code: string;
}

/** Arguments to `commands.authorize`. */
export interface AuthorizeArgs {
  client_id: string;
  response_type: 'code';
  state: string;
  prompt: 'none';
  scope: string[];
}

/** Result of `commands.authenticate` (a superset; we read `user`). */
export interface AuthenticateResult {
  access_token: string;
  user: DiscordUser;
  scopes: string[];
  expires: string;
}

/** Which Discord client hosts the Activity. There is no iOS/Android flag. */
export type Platform = 'desktop' | 'mobile';

/** The structural contract the handshake needs from the SDK. */
export interface SdkLike {
  /** `null` when launched from a DM or group DM. */
  readonly guildId: string | null;
  readonly channelId: string | null;
  readonly platform: Platform;
  /** The `custom_id` of the activity link that launched this instance. */
  readonly customId: string | null;
  /** Resolves once Discord answers the handshake. It never rejects. */
  ready(): Promise<void>;
  commands: {
    authorize(args: AuthorizeArgs): Promise<AuthorizeResult>;
    authenticate(args: { access_token: string }): Promise<AuthenticateResult>;
  };
}
