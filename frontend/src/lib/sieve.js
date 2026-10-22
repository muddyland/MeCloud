/**
 * Mail filter rules ⇄ Sieve (RFC 5228).
 *
 * The rule builder's state is stored in the script itself, as a base64 JSON
 * comment on the first line, so the editor can read back exactly what it
 * wrote. Everything else in the script is generated from that state. Pure, so
 * the quoting — the part that decides whether a rule does what it says — is
 * tested directly.
 */

const HEADERS = { from: 'From', to: 'To', cc: 'Cc', subject: 'Subject' };

/** A Sieve quoted-string body: only `\` and `"` are special (RFC 5228 §2.4.2). */
export function escapeSieveString(value) {
  return String(value ?? '').replace(/\\/g, '\\\\').replace(/"/g, '\\"');
}

/**
 * A literal for `:matches`, where `*` and `?` are wildcards and `\` escapes
 * them. Without this, "starts with a*b" silently became a wildcard rule.
 * Returns the *unquoted* pattern text; quote it with escapeSieveString.
 */
export function escapeMatchLiteral(value) {
  return String(value ?? '').replace(/[\\*?]/g, (c) => `\\${c}`);
}

/** Text that is safe on a `#` comment line: no line breaks, no control chars. */
export function commentSafe(value) {
  // eslint-disable-next-line no-control-regex
  return String(value ?? '').replace(/[\u0000-\u001f\u007f\u2028\u2029]+/g, ' ').trim();
}

const str = (v) => (typeof v === 'string' ? v : v == null ? '' : String(v));

/** The test for one condition, or null if it names a header we do not offer. */
export function buildCondition(c) {
  const h = HEADERS[c?.field];
  if (!h) return null;
  const value = str(c.value);
  const quoted = (s) => `"${escapeSieveString(s)}"`;
  switch (c.op) {
    case 'not_contains': return `not header :contains "${h}" ${quoted(value)}`;
    case 'is':           return `header :is "${h}" ${quoted(value)}`;
    case 'starts_with':  return `header :matches "${h}" ${quoted(`${escapeMatchLiteral(value)}*`)}`;
    case 'ends_with':    return `header :matches "${h}" ${quoted(`*${escapeMatchLiteral(value)}`)}`;
    case 'contains':
    default:             return `header :contains "${h}" ${quoted(value)}`;
  }
}

/** A rule is only written out once every part of it is complete. */
export function isRuleComplete(r) {
  return !!r?.enabled
    && Array.isArray(r.conditions) && r.conditions.length > 0
    && r.conditions.every((c) => str(c?.value).trim() && HEADERS[c?.field])
    && Array.isArray(r.actions) && r.actions.length > 0
    && r.actions.every((a) => a?.type !== 'fileinto' || str(a.mailboxName));
}

export function generateSieve(ruleList) {
  const rules = Array.isArray(ruleList) ? ruleList : [];
  const active = rules.filter(isRuleComplete);
  const needsFlags = active.some((r) =>
    r.actions.some((a) => a.type === 'mark_read' || a.type === 'mark_unread'));

  const reqs = ['fileinto'];
  if (needsFlags) reqs.push('imap4flags');

  const lines = [];
  lines.push(`# @rules ${btoa(encodeURIComponent(JSON.stringify(rules)))}`);
  lines.push(`require [${reqs.map((r) => `"${r}"`).join(', ')}];`);
  lines.push('');

  for (const rule of active) {
    // A line break in the name would end the comment and start script text.
    lines.push(`# Rule: ${commentSafe(rule.name)}`);
    const parts = rule.conditions.map(buildCondition);
    const ifExpr = parts.length === 1
      ? parts[0]
      : `${rule.match === 'any' ? 'anyof' : 'allof'} (\n    ${parts.join(',\n    ')}\n  )`;
    lines.push(`if ${ifExpr} {`);
    for (const a of rule.actions) {
      if      (a.type === 'fileinto' && a.mailboxName) lines.push(`  fileinto "${escapeSieveString(a.mailboxName)}";`);
      else if (a.type === 'mark_read')                 lines.push('  addflag "\\\\Seen";');
      else if (a.type === 'mark_unread')               lines.push('  removeflag "\\\\Seen";');
      else if (a.type === 'discard')                   lines.push('  discard;');
      else if (a.type === 'stop')                      lines.push('  stop;');
    }
    lines.push('}');
    lines.push('');
  }
  return lines.join('\n');
}

/**
 * Read the rule state back out of a script.
 * @returns {{ rules: Array, custom: boolean }} `custom` is true when the
 *          script has content this editor did not write.
 */
export function extractRules(content) {
  const text = String(content ?? '');
  const m = text.match(/^# @rules ([A-Za-z0-9+/=%-]+)/m);
  if (!m) return { rules: [], custom: !!text.trim() };
  try {
    const parsed = JSON.parse(decodeURIComponent(atob(m[1])));
    return { rules: Array.isArray(parsed) ? parsed : [], custom: false };
  } catch {
    return { rules: [], custom: false };
  }
}
