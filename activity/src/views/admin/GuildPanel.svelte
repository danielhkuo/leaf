<script lang="ts">
  import type { AdminApi } from '../../lib/admin/client';
  import type { AdminGuildDetail, AdminSeries } from '../../lib/admin/schemas';
  import InfoTip from '../../lib/components/ui/InfoTip.svelte';

  interface Props {
    api: AdminApi;
    guildId: string;
  }
  let { api, guildId }: Props = $props();

  interface FormState {
    timezone: string;
    creator_role_id: string;
    log_channel_id: string;
    max_series_per_user: number;
    min_account_age_days: number;
    min_membership_age_days: number;
    sprout_enabled: boolean;
    sprout_threshold: number;
  }

  let detail = $state<AdminGuildDetail | null>(null);
  let error = $state('');
  let saving = $state(false);
  let saved = $state(false);
  let form = $state<FormState>({
    timezone: '',
    creator_role_id: '',
    log_channel_id: '',
    max_series_per_user: 0,
    min_account_age_days: 0,
    min_membership_age_days: 0,
    sprout_enabled: false,
    sprout_threshold: 0,
  });

  const errMsg = (e: unknown): string => (e instanceof Error ? e.message : String(e));

  // Load the guild and seed the form from its settings.
  $effect(() => {
    const gid = guildId;
    let cancelled = false;
    detail = null;
    error = '';
    api
      .guild(gid)
      .then((d) => {
        if (cancelled) return;
        detail = d;
        form = {
          timezone: d.settings.timezone,
          creator_role_id: d.settings.creator_role_id ?? '',
          log_channel_id: d.settings.log_channel_id ?? '',
          max_series_per_user: d.settings.max_series_per_user,
          min_account_age_days: d.settings.min_account_age_days,
          min_membership_age_days: d.settings.min_membership_age_days,
          sprout_enabled: d.settings.sprout_enabled,
          sprout_threshold: d.settings.sprout_threshold,
        };
      })
      .catch((e: unknown) => {
        if (!cancelled) error = errMsg(e);
      });
    return () => {
      cancelled = true;
    };
  });

  async function saveSettings(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    saving = true;
    saved = false;
    error = '';
    try {
      const updated = await api.patchSettings(guildId, form);
      if (detail) detail = { ...detail, settings: updated };
      saved = true;
    } catch (err) {
      error = errMsg(err);
    } finally {
      saving = false;
    }
  }

  async function setSeries(
    s: AdminSeries,
    patch: { privacy?: string; state?: string },
  ): Promise<void> {
    error = '';
    try {
      const updated = await api.patchSeries(guildId, s.id, patch);
      if (detail) {
        detail = {
          ...detail,
          series: detail.series.map((x) => (x.id === updated.id ? updated : x)),
        };
      }
    } catch (err) {
      error = errMsg(err);
    }
  }
</script>

{#if error}
  <p class="error" role="alert">{error}</p>
{/if}

{#if !detail}
  <p class="muted pad">Loading server…</p>
{:else}
  <section class="block">
    <p class="eyebrow">Settings</p>
    <form class="settings" onsubmit={saveSettings}>
      <label>
        <span>
          Timezone
          <InfoTip
            label="Timezone"
            text="Time zone for daily reminders and the dates shown in the gallery. Use an IANA name like America/Chicago or Europe/London. Defaults to UTC."
          />
        </span>
        <input type="text" bind:value={form.timezone} placeholder="UTC" />
      </label>
      <label>
        <span>
          Creator role id <em>(blank = anyone)</em>
          <InfoTip
            label="Creator role id"
            text="Restrict who can start a series. With a role ID set, only members who hold that role can create a series in the gallery. Leave blank to let anyone create. (Enable Developer Mode in Discord, then right-click a role → Copy Role ID.)"
          />
        </span>
        <input type="text" bind:value={form.creator_role_id} />
      </label>
      <label>
        <span>
          Log channel id
          <InfoTip
            label="Log channel id"
            text="Channel where leaf posts a short audit line when a series is created, archived, or revoked. Paste a channel ID, or leave blank to turn logging off."
          />
        </span>
        <input type="text" bind:value={form.log_channel_id} />
      </label>
      <label>
        <span>
          Max series per user
          <InfoTip
            label="Max series per user"
            text="How many active series one member may own at once. Example: 3. Must be at least 1."
          />
        </span>
        <input type="number" min="0" bind:value={form.max_series_per_user} />
      </label>
      <label>
        <span>
          Min account age (days)
          <InfoTip
            label="Minimum account age"
            text="Block members whose Discord account is younger than this many days from creating a series — a spam guard. Example: 30. Use 0 to disable."
          />
        </span>
        <input type="number" min="0" bind:value={form.min_account_age_days} />
      </label>
      <label>
        <span>
          Min membership age (days)
          <InfoTip
            label="Minimum membership age"
            text="Block members who joined this server fewer than this many days ago from creating a series. Example: 7. Use 0 to disable."
          />
        </span>
        <input type="number" min="0" bind:value={form.min_membership_age_days} />
      </label>
      <label class="check">
        <input type="checkbox" bind:checked={form.sprout_enabled} />
        <span>
          Sprout probation for new series
          <InfoTip
            label="Sprout probation"
            text="When on, a new series stays hidden as a 🌱 sprout until it reaches the post threshold below, then it appears in the gallery automatically. When off, new series are visible immediately."
          />
        </span>
      </label>
      <label>
        <span>
          Sprout threshold
          <InfoTip
            label="Sprout threshold"
            text="How many posts a sprout needs before it graduates to a normal, visible series. Example: 3. Only applies when sprout probation is on."
          />
        </span>
        <input type="number" min="0" bind:value={form.sprout_threshold} />
      </label>
      <div class="actions">
        <button class="primary" type="submit" disabled={saving}>
          {saving ? 'Saving…' : 'Save settings'}
        </button>
        {#if saved}<span class="ok">Saved</span>{/if}
      </div>
    </form>
  </section>

  <section class="block">
    <p class="eyebrow">
      Series
      <InfoTip
        label="Series controls"
        text="Privacy sets who sees a series in the gallery — Public (everyone in the server), Role-gated (only a chosen role), or Creator only (just the creator and admins). Revoke hides a series and makes it read-only; its archive is kept and you can Restore it later."
      />
    </p>
    {#if detail.series.length === 0}
      <p class="muted">No series yet.</p>
    {:else}
      <ul class="series">
        {#each detail.series as s (s.id)}
          <li class="row" class:revoked={s.state === 'revoked'}>
            <div class="meta">
              <span class="name">{s.name}</span>
              <span class="muted">by {s.creator_id} · {s.state}</span>
            </div>
            <select
              aria-label={`Privacy for ${s.name}`}
              value={s.privacy}
              onchange={(e) => void setSeries(s, { privacy: e.currentTarget.value })}
            >
              <option value="public">Public</option>
              <option value="role_gated">Role-gated</option>
              <option value="creator_only">Creator only</option>
            </select>
            {#if s.state === 'revoked'}
              <button class="ghost" onclick={() => void setSeries(s, { state: 'active' })}>
                Restore
              </button>
            {:else}
              <button class="danger" onclick={() => void setSeries(s, { state: 'revoked' })}>
                Revoke
              </button>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </section>
{/if}

<style>
  .block {
    margin-top: var(--space-lg);
    padding: var(--space-md);
    background: var(--surface-1);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-lg);
  }
  .eyebrow {
    margin: 0 0 var(--space-sm);
    color: var(--ink-subtle);
    font-size: var(--fs-eyebrow);
    font-weight: var(--fw-emphasis);
    letter-spacing: 0.6px;
    text-transform: uppercase;
  }
  .pad {
    padding: var(--space-lg);
  }
  .muted {
    color: var(--ink-muted);
  }
  .error {
    margin: var(--space-md) 0 0;
    padding: var(--space-sm) var(--space-md);
    color: var(--ink);
    background: color-mix(in srgb, var(--error) 25%, var(--surface-1));
    border: 1px solid var(--error);
    border-radius: var(--radius-md);
  }
  .settings {
    display: grid;
    gap: var(--space-md);
    grid-template-columns: 1fr;
  }
  label {
    display: grid;
    gap: 4px;
    font-size: var(--fs-body-sm);
  }
  label em {
    color: var(--ink-subtle);
    font-style: normal;
  }
  label.check {
    grid-auto-flow: column;
    justify-content: start;
    align-items: center;
    gap: var(--space-sm);
  }
  input[type='text'],
  input[type='number'],
  select {
    padding: 10px 12px;
    color: var(--ink);
    font: inherit;
    background: var(--surface-2);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-md);
  }
  input:focus,
  select:focus {
    outline: 2px solid var(--link);
    outline-offset: -1px;
  }
  .actions {
    display: flex;
    gap: var(--space-md);
    align-items: center;
  }
  .ok {
    color: var(--success);
    font-size: var(--fs-body-sm);
  }
  .series {
    display: grid;
    gap: var(--space-xs);
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .row {
    display: grid;
    grid-template-columns: 1fr auto auto;
    gap: var(--space-sm);
    align-items: center;
    padding: var(--space-sm);
    background: var(--surface-2);
    border-radius: var(--radius-md);
  }
  .row.revoked {
    opacity: 0.6;
  }
  .meta {
    display: grid;
    gap: 2px;
    min-width: 0;
  }
  .name {
    font-weight: var(--fw-emphasis);
  }
  .primary {
    padding: 10px 18px;
    color: var(--on-accent-dark);
    font: inherit;
    font-weight: var(--fw-emphasis);
    background: var(--brand);
    border: 0;
    border-radius: var(--radius-md);
    cursor: pointer;
  }
  .primary:disabled {
    opacity: 0.6;
  }
  .ghost {
    padding: 8px 12px;
    color: var(--ink);
    font: inherit;
    font-size: var(--fs-body-sm);
    background: var(--surface-3);
    border: 0;
    border-radius: var(--radius-md);
    cursor: pointer;
  }
  .danger {
    padding: 8px 12px;
    color: var(--error);
    font: inherit;
    font-size: var(--fs-body-sm);
    background: transparent;
    border: 1px solid var(--error);
    border-radius: var(--radius-md);
    cursor: pointer;
  }
  @media (min-width: 640px) {
    .settings {
      grid-template-columns: 1fr 1fr;
    }
    label.check,
    .actions {
      grid-column: 1 / -1;
    }
  }
</style>
