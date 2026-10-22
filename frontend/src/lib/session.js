import { get } from 'svelte/store';
import { jmapSession, jmapAccountId, currentUser } from '$lib/stores/mail.js';
import { getJMAPSession, getAppConfig } from '$lib/api.js';

/**
 * Everything a route needs before it can talk to the server: the app config
 * and the JMAP session, loaded once and shared through the stores.
 *
 * Every route used to carry its own copy of this, and the copies had drifted —
 * two of them dereferenced a null session and swallowed the error into the
 * console, leaving an empty page with no explanation.
 */

let configPromise = null;
let sessionPromise = null;

function config() {
  configPromise ??= getAppConfig().catch(() => ({}));
  return configPromise;
}

/** The first account, whose id every JMAP call is scoped to. */
export function primaryAccountId(session) {
  return Object.keys(session?.accounts ?? {})[0] ?? null;
}

async function loadSession() {
  const session = await getJMAPSession();
  if (!session) return false;                 // apiFetch is already redirecting to login
  const accountId = primaryAccountId(session);
  if (!accountId) throw new Error('This account has no access to any data on the server.');
  jmapSession.set(session);
  jmapAccountId.set(accountId);
  currentUser.set(session.username || session.accounts?.[accountId]?.name || '');
  return true;
}

/**
 * @returns {Promise<{ ready: boolean, error: string, stalwartUrl: string }>}
 *   `ready` is false while a login redirect is under way (nothing to show) or
 *   when `error` says why the session could not be loaded.
 */
export async function ensureSession() {
  const cfg = await config();
  const stalwartUrl = cfg?.stalwartUrl ?? '';
  if (get(jmapAccountId)) return { ready: true, error: '', stalwartUrl };
  sessionPromise ??= loadSession().finally(() => { sessionPromise = null; });
  try {
    const ready = await sessionPromise;
    return { ready, error: '', stalwartUrl };
  } catch (e) {
    return { ready: false, error: e?.message ?? 'Could not reach the server.', stalwartUrl };
  }
}
