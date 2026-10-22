/**
 * Small, pure rules for contact cards.
 */

/** The JSContact properties the contact form edits. */
export const EDITED_CARD_PROPERTIES = [
  'organizations', 'titles', 'emails', 'phones', 'addresses',
  'anniversaries', 'links', 'notes',
];

/**
 * Turn a freshly built card into a correct `/set` update patch.
 *
 * An update is a patch: a property left out is left alone on the server, so a
 * field the user emptied must be sent as null or the old value survives. The
 * one map the form only partly owns is `anniversaries` — it edits the birthday
 * and nothing else — so the other entries are carried over rather than wiped.
 */
export function toCardPatch(card, existing = {}) {
  const patch = { ...card };
  for (const key of EDITED_CARD_PROPERTIES) {
    if (!(key in patch)) patch[key] = null;
  }
  const others = Object.fromEntries(
    Object.entries(existing?.anniversaries ?? {}).filter(([, a]) => a?.kind !== 'birth'),
  );
  if (Object.keys(others).length) {
    patch.anniversaries = { ...others, ...(card.anniversaries ?? {}) };
  }
  return patch;
}

/**
 * A contact's website as something safe to put in an href.
 *
 * The value comes from any imported .vcf, so it is untrusted: only http(s) is
 * linked. A bare host ("example.com") is what people actually type, so it gets
 * https:// rather than being refused.
 */
export function safeWebsiteHref(value) {
  const raw = String(value ?? '').trim();
  if (!raw) return null;
  if (/^https?:\/\//i.test(raw)) return raw;
  if (/^[a-z][a-z0-9+.-]*:/i.test(raw)) return null;          // some other scheme
  if (/^[\w-]+(\.[\w-]+)+([/?#].*)?$/.test(raw)) return `https://${raw}`;
  return null;
}
