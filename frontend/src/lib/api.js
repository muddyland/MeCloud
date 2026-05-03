async function apiFetch(url, options = {}) {
  const res = await fetch(url, { credentials: 'include', ...options });
  if (res.status === 401 && typeof window !== 'undefined') {
    // Session expired — send the user back to the login flow
    window.location.href = '/auth/login';
    return null;
  }
  return res;
}

async function post(methodCalls, using = ['urn:ietf:params:jmap:core', 'urn:ietf:params:jmap:mail']) {
  const res = await apiFetch('/api/jmap', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ using, methodCalls }),
  });
  if (!res) return null;
  if (!res.ok) throw new Error(`JMAP error ${res.status}`);
  return res.json();
}

export async function getJMAPSession() {
  const res = await apiFetch('/api/jmap/session');
  if (!res) return null;
  if (!res.ok) throw new Error(`Session error ${res.status}`);
  return res.json();
}

export async function getAppConfig() {
  const res = await fetch('/api/config');
  if (!res.ok) return { appName: 'JMAP Mail' };
  return res.json();
}

export async function getMe() {
  const res = await apiFetch('/auth/me');
  if (!res) return { authenticated: false };
  if (!res.ok) return { authenticated: false };
  return res.json();
}

export async function logout() {
  window.location.href = '/auth/logout';
}

export async function getIdentities(accountId) {
  const data = await post(
    [['Identity/get', { accountId, ids: null }, 'id']],
    ['urn:ietf:params:jmap:core', 'urn:ietf:params:jmap:submission']
  );
  return data?.methodResponses?.[0]?.[1]?.list ?? [];
}

export function parseAddresses(str) {
  if (!str?.trim()) return [];
  return str.split(',').map(s => {
    s = s.trim();
    const m = s.match(/^(.+?)\s*<([^>]+)>$/);
    return m ? { name: m[1].trim(), email: m[2].trim() } : { email: s };
  }).filter(a => a.email);
}

export async function sendEmail(accountId, identityId, { fromEmail, fromName, to, cc, subject, html, sentMailboxId }) {
  const from = [fromName ? { name: fromName, email: fromEmail } : { email: fromEmail }];
  const toAddrs = parseAddresses(to);
  const ccAddrs = parseAddresses(cc);

  const emailCreate = {
    from,
    to: toAddrs,
    subject: subject || '(no subject)',
    keywords: { '$seen': true },
    bodyValues: { body: { value: html || '' } },
    htmlBody: [{ partId: 'body', type: 'text/html' }],
  };
  if (sentMailboxId) emailCreate.mailboxIds = { [sentMailboxId]: true };
  if (ccAddrs.length) emailCreate.cc = ccAddrs;

  // Step 1: create the email
  const createData = await post(
    [['Email/set', { accountId, create: { draft: emailCreate } }, 'e']],
    ['urn:ietf:params:jmap:core', 'urn:ietf:params:jmap:mail']
  );

  const emailResp = createData?.methodResponses?.[0]?.[1];
  if (emailResp?.notCreated?.draft) {
    throw new Error(emailResp.notCreated.draft.description || 'Failed to create email');
  }

  const emailId = emailResp?.created?.draft?.id;
  if (!emailId) throw new Error('Email was created but server returned no ID');

  // Step 2: submit using the concrete emailId
  const submitData = await post(
    [['EmailSubmission/set', { accountId, create: { send: { identityId, emailId } } }, 's']],
    ['urn:ietf:params:jmap:core', 'urn:ietf:params:jmap:mail', 'urn:ietf:params:jmap:submission']
  );

  const submitResp = submitData?.methodResponses?.[0]?.[1];
  if (submitResp?.notCreated?.send) {
    throw new Error(submitResp.notCreated.send.description || 'Failed to submit email');
  }

  return { emailId };
}

export async function getMailboxes(accountId) {
  const data = await post([
    ['Mailbox/get', { accountId, ids: null }, 'mb']
  ]);
  return data?.methodResponses?.[0]?.[1]?.list ?? [];
}

export async function getEmails(accountId, mailboxId, position = 0, limit = 50) {
  const data = await post([
    [
      'Email/query',
      {
        accountId,
        filter: { inMailbox: mailboxId },
        sort: [{ property: 'receivedAt', isAscending: false }],
        position,
        limit
      },
      'q'
    ],
    [
      'Email/get',
      {
        accountId,
        '#ids': { resultOf: 'q', name: 'Email/query', path: '/ids' },
        properties: ['id', 'subject', 'from', 'receivedAt', 'preview', 'keywords']
      },
      'e'
    ]
  ]);
  return data?.methodResponses?.[1]?.[1]?.list ?? [];
}

export async function destroyEmail(accountId, emailId) {
  const data = await post([
    ['Email/set', { accountId, destroy: [emailId] }, 'del']
  ]);
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notDestroyed?.[emailId]) {
    throw new Error(resp.notDestroyed[emailId].description || 'Delete failed');
  }
}

export async function moveEmail(accountId, emailId, toMailboxId, fromMailboxId) {
  const patch = { [`mailboxIds/${toMailboxId}`]: true };
  if (fromMailboxId) patch[`mailboxIds/${fromMailboxId}`] = null;
  const data = await post([
    ['Email/set', { accountId, update: { [emailId]: patch } }, 'mv']
  ]);
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notUpdated?.[emailId]) {
    throw new Error(resp.notUpdated[emailId].description || 'Move failed');
  }
}

export async function renameMailbox(accountId, mailboxId, newName) {
  const data = await post([
    ['Mailbox/set', { accountId, update: { [mailboxId]: { name: newName } } }, 'mb']
  ]);
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notUpdated?.[mailboxId]) {
    throw new Error(resp.notUpdated[mailboxId].description || 'Rename failed');
  }
}

export async function deleteMailbox(accountId, mailboxId) {
  const data = await post([
    ['Mailbox/set', { accountId, destroy: [mailboxId] }, 'mb']
  ]);
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notDestroyed?.[mailboxId]) {
    throw new Error(resp.notDestroyed[mailboxId].description || 'Delete failed');
  }
}

export async function createMailbox(accountId, name, parentId = null) {
  const props = { name, role: null };
  if (parentId) props.parentId = parentId;
  const data = await post([
    ['Mailbox/set', { accountId, create: { newMailbox: props } }, 'mb']
  ]);
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notCreated?.newMailbox) {
    throw new Error(resp.notCreated.newMailbox.description || 'Create failed');
  }
  const created = resp?.created?.newMailbox ?? null;
  if (!created) return null;
  // JMAP only returns server-assigned fields in `created`; merge with what we sent
  return { ...props, ...created };
}

export async function markEmailSeen(accountId, emailId, seen) {
  const data = await post([
    ['Email/set', { accountId, update: { [emailId]: { 'keywords/$seen': seen ? true : null } } }, 'mark']
  ]);
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notUpdated?.[emailId]) {
    throw new Error(resp.notUpdated[emailId].description || 'Mark failed');
  }
}

function appPasswordUsing(session) {
  // Use every capability the server advertises — avoids guessing the exact URI
  // for Stalwart's x:AppPassword extension.
  const caps = Object.keys(session?.capabilities ?? {});
  return caps.length ? caps : ['urn:ietf:params:jmap:core'];
}

export async function getAppPasswords(accountId, session) {
  const using = appPasswordUsing(session);
  const data = await post([
    ['x:AppPassword/query', { accountId }, '0'],
    ['x:AppPassword/get', {
      accountId,
      '#ids': { resultOf: '0', name: 'x:AppPassword/query', path: '/ids' },
    }, '1'],
  ], using);
  return data?.methodResponses?.[1]?.[1]?.list ?? [];
}

export async function createAppPassword(accountId, description, session) {
  const using = appPasswordUsing(session);
  const data = await post([
    ['x:AppPassword/set', {
      accountId,
      create: { new: { description, expiresAt: null } },
    }, '0'],
  ], using);
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notCreated?.new) {
    throw new Error(resp.notCreated.new.description || 'Failed to create app password');
  }
  return resp?.created?.new ?? null; // { id, secret, ... }
}

export async function deleteAppPassword(accountId, id, session) {
  const using = appPasswordUsing(session);
  const data = await post([
    ['x:AppPassword/set', { accountId, destroy: [id] }, '0'],
  ], using);
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notDestroyed?.[id]) {
    throw new Error(resp.notDestroyed[id].description || 'Failed to delete app password');
  }
}

export async function getSieveScript() {
  const res = await apiFetch('/api/sieve');
  if (!res) return null;
  if (res.status === 503) return null;
  if (!res.ok) throw new Error(`Sieve error ${res.status}`);
  return res.json();
}

export async function saveSieveScript(id, name, content, makeActive = true) {
  const res = await apiFetch('/api/sieve', {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ id, name, content, makeActive }),
  });
  if (!res) return null;
  if (!res.ok) throw new Error(`Sieve save error ${res.status}`);
  return res.json();
}

export async function getEmailBody(accountId, emailId) {
  const data = await post([
    [
      'Email/get',
      {
        accountId,
        ids: [emailId],
        properties: ['id', 'subject', 'from', 'to', 'cc', 'receivedAt', 'keywords', 'htmlBody', 'textBody', 'bodyValues'],
        fetchHTMLBodyValues: true,
        fetchTextBodyValues: true
      },
      'e'
    ]
  ]);
  return data?.methodResponses?.[0]?.[1]?.list?.[0] ?? null;
}
