/**
 * Pure helpers for the message list.
 */

/**
 * Append a page of messages, skipping ids already present.
 *
 * Paging is by position, so a message that arrives between two page loads
 * shifts everything down by one and the next page repeats the last message of
 * the previous one. The list is keyed by id, and a duplicate key is a hard
 * error in Svelte, so the repeat must never reach it.
 */
export function appendUnique(list, more) {
  const seen = new Set(list.map((e) => e.id));
  const fresh = more.filter((e) => e && !seen.has(e.id) && seen.add(e.id));
  return fresh.length ? [...list, ...fresh] : list;
}

/**
 * Attach `threadSize` to each message from a Thread/get response list, and
 * `threadMembers` — every message of the thread as {id, mailboxIds, from,
 * receivedAt}, newest first — when the members were fetched. A message whose
 * thread did not come back keeps a size of 1 and no members.
 */
export function withThreadSizes(emails, threads, members) {
  const sizes = new Map((threads ?? []).map((t) => [t.id, t.emailIds?.length ?? 1]));
  const byThread = new Map();
  for (const m of members ?? []) {
    if (!m?.threadId) continue;
    if (!byThread.has(m.threadId)) byThread.set(m.threadId, []);
    byThread.get(m.threadId).push(m);
  }
  for (const list of byThread.values()) {
    list.sort((a, b) => String(b.receivedAt ?? '').localeCompare(String(a.receivedAt ?? '')));
  }
  return emails.map((e) => {
    const out = { ...e, threadSize: sizes.get(e.threadId) ?? 1 };
    if (byThread.has(e.threadId)) out.threadMembers = byThread.get(e.threadId);
    return out;
  });
}

// ── Conversations ───────────────────────────────────────────────────────────
//
// The store keeps a flat list of messages, newest first, exactly as the server
// paged them. Conversations are a view over it: every action still names
// concrete message ids, so a conversation card acting on "all of it" means the
// messages it actually holds in this list, never a guess about the thread.

const isUnread = (e) => !e?.keywords?.['$seen'];

/**
 * Group a newest-first message list into conversations, in the order their
 * newest message appears. Each group carries the messages this list holds
 * (newest first) and `size`, the whole thread's length as the server reported
 * it — which counts copies in other folders, such as your replies in Sent.
 */
export function groupByThread(list) {
  const groups = [];
  const byKey = new Map();
  for (const email of list ?? []) {
    if (!email) continue;
    const key = email.threadId ?? `solo:${email.id}`;
    let group = byKey.get(key);
    if (!group) {
      group = { key, threadId: email.threadId ?? null, latest: email, emails: [], size: 1, unread: 0, members: null };
      byKey.set(key, group);
      groups.push(group);
    }
    group.emails.push(email);
    if (!group.members && email.threadMembers?.length) group.members = email.threadMembers;
    group.size = Math.max(group.size, email.threadSize ?? 1, group.emails.length);
    if (isUnread(email)) group.unread += 1;
  }
  return groups;
}

/** The message ids a conversation card acts on: the ones in this list. */
export function groupIds(group) {
  return (group?.emails ?? []).map((e) => e.id);
}

/**
 * Every message to show under an expanded card, newest first.
 *
 * `threadEmails` is the full thread from Thread/get, which includes messages
 * outside this folder. Where a message is also in the list, the list's copy
 * wins: it is the one that mark-read and refreshes keep current. Without the
 * full thread (not loaded yet, or the fetch failed) the list's own messages
 * are shown, so expanding never comes up empty.
 */
export function conversationMessages(group, threadEmails) {
  const own = new Map((group?.emails ?? []).map((e) => [e.id, e]));
  const source = threadEmails?.length ? threadEmails : [...own.values()];
  const merged = source.map((e) => own.get(e.id) ?? e);
  for (const e of own.values()) if (!merged.some((m) => m.id === e.id)) merged.push(e);
  return merged.sort((a, b) => String(b.receivedAt ?? '').localeCompare(String(a.receivedAt ?? '')));
}

/**
 * The people in a conversation, newest first and de-duplicated, with your own
 * address shown as "Me" — what the collapsed card names.
 */
export function participants(messages, me = '') {
  const self = String(me).toLowerCase();
  const out = [];
  const seen = new Set();
  for (const m of messages ?? []) {
    const f = m?.from?.[0];
    if (!f) continue;
    const email = String(f.email ?? '').toLowerCase();
    const label = email && email === self ? 'Me' : (f.name || f.email || 'Unknown');
    const id = email || label;
    if (seen.has(id)) continue;
    seen.add(id);
    out.push(label);
  }
  return out;
}

/**
 * The order j/k walks: one stop per collapsed conversation (its newest
 * message), and every message of an expanded one.
 */
export function navigationOrder(groups, expanded, threads, keep = () => true) {
  const out = [];
  for (const g of groups ?? []) {
    if (g.size > 1 && expanded?.has(g.key)) {
      out.push(...conversationMessages(g, threads?.get(g.key)?.list).filter(keep));
    } else {
      out.push(g.latest);
    }
  }
  return out;
}

/**
 * Whether a message belongs in an expanded conversation viewed from
 * `currentMailboxId`. Copies that live only in Trash or Junk are left out —
 * deleting a reply should not keep it in the conversation — unless that is
 * the folder being viewed. A message with no mailbox information (a list or
 * search result) is always shown.
 */
export function shownInConversation(message, currentMailboxId, hiddenMailboxIds) {
  const ids = Object.keys(message?.mailboxIds ?? {});
  if (!ids.length) return true;
  if (currentMailboxId && ids.includes(currentMailboxId)) return true;
  return ids.some((id) => !hiddenMailboxIds?.has(id));
}

/**
 * What a conversation card shows before it is opened: how many messages and
 * whose, counting exactly what expanding it will list — the thread's members
 * the view keeps — or, without member data, the thread size and the senders
 * this list holds.
 */
export function cardSummary(group, keep = () => true, me = '') {
  const shown = group?.members ? group.members.filter(keep) : null;
  return {
    count: shown ? Math.max(shown.length, 1) : (group?.size ?? 1),
    people: participants(shown?.length ? shown : group?.emails, me),
  };
}
