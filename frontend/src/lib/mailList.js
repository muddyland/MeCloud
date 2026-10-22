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
 * Attach `threadSize` to each message from a Thread/get response list.
 * A message whose thread did not come back keeps a size of 1.
 */
export function withThreadSizes(emails, threads) {
  const sizes = new Map((threads ?? []).map((t) => [t.id, t.emailIds?.length ?? 1]));
  return emails.map((e) => ({ ...e, threadSize: sizes.get(e.threadId) ?? 1 }));
}
