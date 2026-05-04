import { get } from 'svelte/store';
import { mailboxes, jmapAccountId } from '$lib/stores/mail.js';
import { getMailboxes } from '$lib/api.js';

// Fetch fresh unread counts from the server and merge them into the mailboxes store,
// preserving the sidebar sort order established at startup.
export async function refreshMailboxCounts() {
  const accountId = get(jmapAccountId);
  if (!accountId) return;
  try {
    const fresh = await getMailboxes(accountId);
    mailboxes.update(existing => existing.map(mb => {
      const updated = fresh.find(f => f.id === mb.id);
      return updated ? { ...mb, unreadEmails: updated.unreadEmails ?? 0 } : mb;
    }));
  } catch {}
}
