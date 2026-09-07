/**
 * Attachment selection rules.
 *
 * Pure, so the decisions that are easy to get wrong — refusing a duplicate,
 * staying inside the size budget — can be tested directly rather than through
 * the compose modal.
 */

/**
 * Choose which drive files may be attached to a message.
 *
 * Files already in the drive need no upload, so a selection either becomes a
 * finished attachment row or is refused outright; there is no in-between state
 * to report. Two things refuse one:
 *
 *  - it is already on the message. The picker greys those out, but the same
 *    file is reachable twice through search, and a batch can also name one
 *    twice, so the blob id is the authority rather than the UI.
 *  - it would push the message past `maxTotalBytes`. The budget is spent in
 *    the order given, so an oversized file does not starve the smaller ones
 *    behind it.
 *
 * @param {object[]} nodes                FileNodes chosen in the picker
 * @param {Set<string>} opts.attachedBlobIds  blob ids already on the message
 * @param {number} opts.usedBytes         bytes those already account for
 * @param {number} opts.maxTotalBytes     ceiling for the whole message
 * @returns {{ rows: object[], skipped: number, duplicates: number }}
 */
export function selectDriveAttachments(nodes, {
  attachedBlobIds = new Set(),
  usedBytes = 0,
  maxTotalBytes = Infinity,
} = {}) {
  const taken = new Set(attachedBlobIds);
  const rows = [];
  let running = usedBytes;
  let skipped = 0;
  let duplicates = 0;

  for (const node of nodes ?? []) {
    // A folder has no blob, and neither does a node the server never echoed.
    if (!node?.blobId) continue;
    if (taken.has(node.blobId)) { duplicates += 1; continue; }

    const size = Number(node.size);
    const bytes = Number.isFinite(size) && size > 0 ? size : 0;
    if (running + bytes > maxTotalBytes) { skipped += 1; continue; }

    running += bytes;
    taken.add(node.blobId);
    rows.push({
      name: node.name ?? 'attachment',
      type: node.type || 'application/octet-stream',
      size: bytes,
      blobId: node.blobId,
    });
  }

  return { rows, skipped, duplicates };
}
