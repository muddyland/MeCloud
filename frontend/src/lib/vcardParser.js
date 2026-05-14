function unfold(text) {
  return text.replace(/\r\n[ \t]/g, '').replace(/\n[ \t]/g, '');
}

function decodeQuotedPrintable(str) {
  // Decode soft line breaks first, then hex sequences
  return str
    .replace(/=\r?\n/g, '')
    .replace(/=([0-9A-Fa-f]{2})/g, (_, hex) => String.fromCharCode(parseInt(hex, 16)));
}

function parseLine(raw) {
  const colonIdx = raw.indexOf(':');
  if (colonIdx === -1) return null;

  const namePart = raw.slice(0, colonIdx);
  const value = raw.slice(colonIdx + 1);

  // Strip group prefix (e.g. "item1.EMAIL" → "EMAIL")
  const dotIdx = namePart.indexOf('.');
  const effective = dotIdx !== -1 ? namePart.slice(dotIdx + 1) : namePart;

  const segments = effective.split(';');
  const propName = segments[0].toUpperCase();

  const params = {};
  for (let i = 1; i < segments.length; i++) {
    const eq = segments[i].indexOf('=');
    if (eq === -1) continue;
    const key = segments[i].slice(0, eq).toUpperCase();
    const val = segments[i].slice(eq + 1).toUpperCase();
    if (key === 'TYPE') {
      params.TYPE = params.TYPE ? `${params.TYPE},${val}` : val;
    } else {
      params[key] = val;
    }
  }

  return { name: propName, params, value };
}

function getTypes(params) {
  return (params.TYPE ?? '').split(',').map(t => t.trim().toLowerCase()).filter(Boolean);
}

function emailContext(types) {
  if (types.includes('work')) return 'work';
  if (types.includes('home')) return 'personal';
  return 'work';
}

function phoneContext(types) {
  if (types.includes('cell') || types.includes('mobile')) return 'mobile';
  if (types.includes('work')) return 'work';
  if (types.includes('home')) return 'home';
  return 'mobile';
}

function parseDate(dateStr) {
  const s = dateStr.trim();

  // --MMDD or --MM-DD (no year)
  if (s.startsWith('--')) {
    const rest = s.slice(2).replace(/-/g, '');
    if (rest.length >= 4) {
      return {
        month: parseInt(rest.slice(0, 2)) || undefined,
        day:   parseInt(rest.slice(2, 4)) || undefined,
      };
    }
    return null;
  }

  // YYYYMMDD or YYYY-MM-DD
  const clean = s.split('T')[0].replace(/-/g, '');
  if (clean.length >= 8) {
    const year  = parseInt(clean.slice(0, 4));
    const month = parseInt(clean.slice(4, 6));
    const day   = parseInt(clean.slice(6, 8));
    return {
      ...(year  ? { year  } : {}),
      ...(month ? { month } : {}),
      ...(day   ? { day   } : {}),
    };
  }

  return null;
}

function buildContact(props) {
  const card = { '@type': 'Card', version: '1.0', name: { full: '' } };
  let emailIdx = 0;
  let phoneIdx = 0;
  let hasOrg = false;

  for (const { name, params, value } of props) {
    const encoding = params.ENCODING ?? '';
    const actual = encoding === 'QUOTED-PRINTABLE' ? decodeQuotedPrintable(value) : value;

    // Skip binary blobs
    if (encoding === 'BASE64' || encoding === 'B') continue;

    switch (name) {
      case 'FN':
        card.name = { full: actual.trim() };
        break;

      case 'N':
        // N:Last;First;Middle;Prefix;Suffix — only use if FN wasn't set
        if (!card.name.full) {
          const parts = actual.split(';');
          const last  = parts[0]?.trim() ?? '';
          const first = parts[1]?.trim() ?? '';
          card.name = { full: [first, last].filter(Boolean).join(' ') };
        }
        break;

      case 'EMAIL': {
        const ctx = emailContext(getTypes(params));
        const addr = actual.trim();
        if (!addr) break;
        if (!card.emails) card.emails = {};
        card.emails[`e${++emailIdx}`] = {
          '@type': 'EmailAddress',
          address: addr,
          contexts: { [ctx]: true },
        };
        break;
      }

      case 'TEL': {
        const ctx = phoneContext(getTypes(params));
        const num = actual.trim();
        if (!num) break;
        if (!card.phones) card.phones = {};
        card.phones[`p${++phoneIdx}`] = {
          '@type': 'Phone',
          number: num,
          contexts: { [ctx]: true },
        };
        break;
      }

      case 'ORG': {
        const orgName = actual.split(';')[0].trim();
        if (!orgName) break;
        card.organizations = { o1: { '@type': 'Organization', name: orgName } };
        hasOrg = true;
        break;
      }

      case 'TITLE': {
        const title = actual.trim();
        if (!title) break;
        card.titles = { t1: { '@type': 'Title', name: title } };
        if (hasOrg) card.titles.t1.organizationId = 'o1';
        break;
      }

      case 'ADR': {
        // PO Box ; Extended ; Street ; City ; Region ; Postal ; Country
        const p = actual.split(';');
        const streetParts = [p[2], p[1], p[0]].map(s => s?.trim()).filter(Boolean);
        const street  = streetParts.join(', ');
        const city    = p[3]?.trim() ?? '';
        const region  = p[4]?.trim() ?? '';
        const postal  = p[5]?.trim() ?? '';
        const country = p[6]?.trim() ?? '';
        if (!street && !city && !region && !postal && !country) break;
        card.addresses = { a1: {
          '@type': 'Address',
          ...(street ? { components: [{ '@type': 'AddressComponent', kind: 'name', value: street }] } : {}),
          ...(city   ? { locality: city   } : {}),
          ...(region ? { region           } : {}),
          ...(postal ? { postcode: postal } : {}),
          ...(country? { country          } : {}),
        }};
        break;
      }

      case 'BDAY': {
        const date = parseDate(actual);
        if (!date) break;
        card.anniversaries = { birth: {
          '@type': 'Anniversary', kind: 'birth',
          date: { '@type': 'PartialDate', ...date },
        }};
        break;
      }

      case 'URL':
      case 'LINK': {
        const href = actual.trim();
        if (!href) break;
        card.links = { w1: { '@type': 'Link', href } };
        break;
      }

      case 'NOTE': {
        const note = actual.trim();
        if (!note) break;
        card.notes = { n1: { '@type': 'Note', note } };
        break;
      }
    }
  }

  return card;
}

/**
 * Parse a .vcf file text into an array of JSContact Card objects.
 * Supports vCard 2.1, 3.0, and 4.0; handles multiple vCARDs per file.
 */
export function parseVCard(text) {
  const unfolded = unfold(text);
  const lines = unfolded.split(/\r?\n/);

  const contacts = [];
  let currentProps = null;

  for (const raw of lines) {
    const line = raw.trim();
    if (!line) continue;

    const upper = line.toUpperCase();
    if (upper === 'BEGIN:VCARD') {
      currentProps = [];
      continue;
    }
    if (upper === 'END:VCARD') {
      if (currentProps) contacts.push(buildContact(currentProps));
      currentProps = null;
      continue;
    }
    if (!currentProps) continue;

    const parsed = parseLine(line);
    if (parsed) currentProps.push(parsed);
  }

  return contacts.filter(c => c.name.full);
}
