import { describe, it, expect, vi, beforeEach } from 'vitest';
import {
  INTEROPERABLE_PERMISSIONS, isForbidden, refusedPermissions, replacePermissions,
} from '$lib/stalwartPermissions.js';

/*
 * Stalwart 1.0.3 scopes OAuth tokens and refuses an app password that would
 * inherit more than the token holds. These fakes answer the way the servers
 * were observed to (MeCloud's token: interoperable scopes + security).
 */
const REFUSAL = {
  type: 'forbidden',
  description: 'You are not authorized to grant permissions: interactAi, sysAccountSettingsUpdate, sysAccountSettingsGet',
};

function fakeServer(answers) {
  const sent = [];
  fetch.mockImplementation(async (_url, init) => {
    const body = JSON.parse(init.body);
    const props = body.methodCalls[0][1].create.new;
    sent.push(props);
    const answer = answers(props, sent.length);
    return { status: 200, ok: true, json: async () => ({ methodResponses: [['x:AppPassword/set', answer, '0']] }) };
  });
  return sent;
}

const created = { created: { new: { id: 'p1', secret: 's3cret' } } };

describe('createAppPassword', () => {
  beforeEach(() => {
    vi.stubGlobal('window', { location: { href: '' } });
    vi.stubGlobal('fetch', vi.fn());
  });

  it('asks for the account permissions first (Stalwart 0.16)', async () => {
    const sent = fakeServer(() => created);
    const { createAppPassword } = await import('$lib/api.js');
    expect(await createAppPassword('a', 'Thunderbird', {})).toEqual({ id: 'p1', secret: 's3cret' });
    expect(sent).toEqual([{ description: 'Thunderbird', expiresAt: null }]);
  });

  it('lists the interoperable permissions when a scoped token is refused (1.0.3)', async () => {
    const sent = fakeServer((props) => (props.permissions ? created : { notCreated: { new: REFUSAL } }));
    const { createAppPassword } = await import('$lib/api.js');
    expect((await createAppPassword('a', 'Phone', {})).secret).toBe('s3cret');
    expect(sent).toHaveLength(2);
    expect(sent[1].permissions['@type']).toBe('Replace');
    expect(Object.keys(sent[1].permissions.permissions)).toEqual([...INTEROPERABLE_PERMISSIONS]);
  });

  it('drops permissions the account lacks, once', async () => {
    const sent = fakeServer((props, n) => {
      if (n === 1) return { notCreated: { new: REFUSAL } };
      if (n === 2) return { notCreated: { new: { type: 'forbidden', description: 'You are not authorized to grant permissions: calendarAlarmsSend, davCalAcl' } } };
      return created;
    });
    const { createAppPassword } = await import('$lib/api.js');
    expect((await createAppPassword('a', 'Phone', {})).id).toBe('p1');
    expect(sent).toHaveLength(3);
    const granted = sent[2].permissions.permissions;
    expect(granted.calendarAlarmsSend).toBeUndefined();
    expect(granted.davCalAcl).toBeUndefined();
    expect(granted.jmapEmailGet).toBe(true);
  });

  it('reports a refusal it cannot work around', async () => {
    const sent = fakeServer(() => ({ notCreated: { new: { type: 'forbidden', description: 'Quota exceeded' } } }));
    const { createAppPassword } = await import('$lib/api.js');
    await expect(createAppPassword('a', 'Phone', {})).rejects.toThrow();
    expect(sent).toHaveLength(2);
  });

  it('does not retry other errors', async () => {
    const sent = fakeServer(() => ({ notCreated: { new: { type: 'invalidProperties', properties: ['description'] } } }));
    const { createAppPassword } = await import('$lib/api.js');
    await expect(createAppPassword('a', 'x', {})).rejects.toThrow();
    expect(sent).toHaveLength(1);
  });
});

describe('stalwartPermissions helpers', () => {
  it('parses the refused permissions', () => {
    expect([...refusedPermissions(REFUSAL)]).toEqual(['interactAi', 'sysAccountSettingsUpdate', 'sysAccountSettingsGet']);
    expect(refusedPermissions({ type: 'forbidden', description: 'nope' }).size).toBe(0);
    expect(refusedPermissions({ type: 'invalidProperties' }).size).toBe(0);
    expect(isForbidden(null)).toBe(false);
  });

  it('keeps account-management permissions out of the set', () => {
    for (const p of ['sysAppPasswordCreate', 'sysAccountPasswordUpdate', 'sysAccountSettingsUpdate', 'sysApiKeyCreate']) {
      expect(INTEROPERABLE_PERMISSIONS).not.toContain(p);
    }
    expect(new Set(INTEROPERABLE_PERMISSIONS).size).toBe(INTEROPERABLE_PERMISSIONS.length);
    expect(replacePermissions(new Set(['authenticate'])).permissions.authenticate).toBeUndefined();
  });
});
