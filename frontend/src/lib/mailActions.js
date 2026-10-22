import { get } from 'svelte/store';
import {
  emails, selectedEmailId, selectedEmailIds, jmapAccountId, mailboxes,
  selectedMailbox, mailRefresher,
} from '$lib/stores/mail.js';
import { bulkMove, bulkDestroy } from '$lib/api.js';
import { refreshMailboxCounts } from '$lib/mailboxRefresh.js';
import { toast, undoToast } from '$lib/stores/toast.js';

/**
 * Moving and deleting mail, in one place.
 *
 * Five components used to each carry their own copy of "call the server, drop
 * the rows, fix the selection, refresh the counts, say so". One copy is what
 * makes Undo possible everywhere at once: a move is undone by moving back,
 * which needs nothing but the ids and the two mailboxes.
 */

const plural = (n) => `${n} message${n === 1 ? '' : 's'}`;

function dropFromList(ids) {
  const idSet = new Set(ids);
  emails.update((list) => list.filter((e) => !idSet.has(e.id)));
  if (idSet.has(get(selectedEmailId))) selectedEmailId.set(null);
  const picked = get(selectedEmailIds);
  if ([...picked].some((id) => idSet.has(id))) selectedEmailIds.set(new Set());
}

/**
 * Move messages to `to`, offering Undo.
 *
 * @param {string[]} ids
 * @param {{id: string, name?: string}} to
 * @param {string|null} fromId  the mailbox they are being moved out of
 * @param {string} [message]    the toast text, if not "Moved to <name>"
 */
export async function moveMessages(ids, to, fromId, message) {
  if (!ids.length) return;
  const accountId = get(jmapAccountId);
  await bulkMove(accountId, ids, to.id, fromId);
  dropFromList(ids);
  await refreshMailboxCounts();

  const text = message ?? (ids.length === 1
    ? `Moved to ${to.name}` : `${plural(ids.length)} moved to ${to.name}`);
  if (!fromId) { toast(text, 'success'); return; }

  undoToast(text, async () => {
    await bulkMove(accountId, ids, fromId, to.id);
    await refreshMailboxCounts();
    // Back in the folder on screen: let the list show them again.
    if (get(selectedMailbox)?.id === fromId) await get(mailRefresher)?.();
  });
}

/**
 * Delete messages: to Trash with Undo, or permanently when already in Trash
 * (which is the one thing here that cannot be undone, and says so).
 *
 * `from` is the mailbox they are in — the folder on screen unless the caller
 * knows better (a message opened from a conversation may live in Sent).
 */
export async function deleteMessages(ids, from = get(selectedMailbox)) {
  if (!ids.length) return;
  const current = from;
  if (current?.role === 'trash') {
    await bulkDestroy(get(jmapAccountId), ids);
    dropFromList(ids);
    await refreshMailboxCounts();
    toast(`${ids.length === 1 ? 'Message' : plural(ids.length)} deleted permanently`, 'success');
    return;
  }
  const trash = get(mailboxes).find((m) => m.role === 'trash');
  if (!trash) throw new Error('No Trash folder on this account.');
  await moveMessages(ids, trash, current?.id ?? null,
    ids.length === 1 ? 'Moved to Trash' : `${plural(ids.length)} moved to Trash`);
}
