<script>
  import { contactModalOpen, editingContact, contacts, selectedAddressBook, addressBooks, selectedContact } from '$lib/stores/contacts.js';
  import { jmapAccountId, jmapSession } from '$lib/stores/mail.js';
  import { createContact, updateContact, deleteContact } from '$lib/api.js';
  import { toast } from '$lib/stores/toast.js';

  let saving   = false;
  let deleting = false;

  let fullName  = '';
  let jobTitle  = '';
  let org       = '';
  let birthday  = '';
  let website   = '';
  let notes     = '';
  let street    = '';
  let city      = '';
  let state     = '';
  let postal    = '';
  let country   = '';

  // Dynamic lists — each entry has value + label/type
  let emailList = [{ address: '', type: 'work' }];
  let phoneList = [{ number: '', type: 'mobile' }];

  $: if ($contactModalOpen) init();

  function init() {
    const c = $editingContact;
    if (c) {
      fullName = c.name?.full ?? '';
      jobTitle = Object.values(c.titles ?? {})[0]?.name ?? '';
      org      = Object.values(c.organizations ?? {})[0]?.name ?? '';
      website  = Object.values(c.links ?? {})[0]?.href ?? '';
      notes    = Object.values(c.notes ?? {})[0]?.note ?? '';

      emailList = Object.values(c.emails ?? {}).map(e => ({
        address: e.address ?? '',
        type: Object.keys(e.contexts ?? {})[0] ?? 'work',
      }));
      if (!emailList.length) emailList = [{ address: '', type: 'work' }];

      phoneList = Object.values(c.phones ?? {}).map(p => ({
        number: p.number ?? '',
        type: Object.keys(p.contexts ?? {})[0] ?? 'mobile',
      }));
      if (!phoneList.length) phoneList = [{ number: '', type: 'mobile' }];

      const addr = Object.values(c.addresses ?? {})[0] ?? {};
      const components = Array.isArray(addr.components) ? addr.components : [];
      street  = components.find(s => s.kind === 'name')?.value ?? '';
      city    = addr.locality  ?? '';
      state   = addr.region    ?? '';
      postal  = addr.postcode  ?? '';
      country = addr.country   ?? '';

      const birth = Object.values(c.anniversaries ?? {}).find(a => a.kind === 'birth');
      if (birth?.date) {
        const { year, month, day } = birth.date;
        birthday = [
          year  ? String(year).padStart(4,'0') : '0000',
          month ? String(month).padStart(2,'0') : '01',
          day   ? String(day).padStart(2,'0')   : '01',
        ].join('-');
      } else {
        birthday = '';
      }
    } else {
      fullName = jobTitle = org = website = notes = birthday = '';
      street = city = state = postal = country = '';
      emailList = [{ address: '', type: 'work' }];
      phoneList = [{ number: '', type: 'mobile' }];
    }
  }

  function addEmail() { emailList = [...emailList, { address: '', type: 'work' }]; }
  function removeEmail(i) { emailList = emailList.filter((_, idx) => idx !== i); }
  function addPhone() { phoneList = [...phoneList, { number: '', type: 'mobile' }]; }
  function removePhone(i) { phoneList = phoneList.filter((_, idx) => idx !== i); }

  function buildCard() {
    const card = { '@type': 'Card', version: '1.0', name: { full: fullName.trim() } };

    if (org.trim()) card.organizations = { o1: { '@type': 'Organization', name: org.trim() } };

    if (jobTitle.trim()) {
      card.titles = { t1: { '@type': 'Title', name: jobTitle.trim() } };
      if (org.trim()) card.titles.t1.organizationId = 'o1';
    }

    const validEmails = emailList.filter(e => e.address.trim());
    if (validEmails.length) {
      card.emails = {};
      validEmails.forEach((e, i) => {
        card.emails[`e${i+1}`] = { '@type': 'EmailAddress', address: e.address.trim(), contexts: { [e.type]: true } };
      });
    }

    const validPhones = phoneList.filter(p => p.number.trim());
    if (validPhones.length) {
      card.phones = {};
      validPhones.forEach((p, i) => {
        card.phones[`p${i+1}`] = { '@type': 'Phone', number: p.number.trim(), contexts: { [p.type]: true } };
      });
    }

    if (street.trim() || city.trim() || state.trim() || postal.trim() || country.trim()) {
      card.addresses = { a1: {
        '@type': 'Address',
        ...(street.trim() ? { components: [{ '@type': 'AddressComponent', kind: 'name', value: street.trim() }] } : {}),
        ...(city.trim()   ? { locality: city.trim()   } : {}),
        ...(state.trim()  ? { region:   state.trim()  } : {}),
        ...(postal.trim() ? { postcode: postal.trim() } : {}),
        ...(country.trim()? { country:  country.trim()} : {}),
      }};
    }

    if (birthday.trim()) {
      const [y, m, d] = birthday.split('-').map(Number);
      if (y || m || d) {
        card.anniversaries = { birth: {
          '@type': 'Anniversary', kind: 'birth',
          date: { '@type': 'PartialDate', ...(y ? { year: y } : {}), ...(m ? { month: m } : {}), ...(d ? { day: d } : {}) },
        }};
      }
    }

    if (website.trim()) card.links = { w1: { '@type': 'Link', href: website.trim() } };
    if (notes.trim())   card.notes = { n1: { '@type': 'Note', note: notes.trim() } };

    return card;
  }

  function close() {
    contactModalOpen.set(false);
    editingContact.set(null);
  }

  async function save() {
    if (!fullName.trim()) return;
    saving = true;
    try {
      const card = buildCard();
      if ($editingContact) {
        await updateContact($jmapAccountId, $jmapSession, $editingContact.id, card);
        const updated = { ...$editingContact, ...card };
        contacts.update(list => list.map(c => c.id === $editingContact.id ? updated : c));
        selectedContact.set(updated);
        toast('Contact updated', 'success');
      } else {
        const abId = $selectedAddressBook ?? $addressBooks[0]?.id;
        const created = await createContact($jmapAccountId, $jmapSession, card, abId);
        if (created) contacts.update(list => [...list, { ...card, id: created.id }]);
        toast('Contact created', 'success');
      }
      close();
    } catch (e) {
      toast(e?.message ?? 'Save failed', 'error');
    } finally {
      saving = false;
    }
  }

  async function remove() {
    if (!$editingContact) return;
    deleting = true;
    try {
      await deleteContact($jmapAccountId, $jmapSession, $editingContact.id);
      contacts.update(list => list.filter(c => c.id !== $editingContact.id));
      selectedContact.set(null);
      toast('Contact deleted', 'success');
      close();
    } catch (e) {
      toast(e?.message ?? 'Delete failed', 'error');
    } finally {
      deleting = false;
    }
  }

  const inputCls = 'text-sm px-3 py-2 rounded-lg border border-gray-200 dark:border-gray-600 bg-white dark:bg-gray-700 text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-blue-500';
  const labelCls = 'block text-xs font-medium text-gray-500 dark:text-gray-400 mb-1';
  const sectionCls = 'text-xs font-semibold text-gray-400 dark:text-gray-500 uppercase tracking-wider pt-1';
</script>

{#if $contactModalOpen}
  <div class="fixed inset-0 bg-black/40 z-40 flex items-center justify-center p-4"
       on:click|self={close} role="dialog" aria-modal="true">

    <div class="w-full max-w-lg bg-white dark:bg-gray-800 rounded-2xl shadow-xl z-50 flex flex-col max-h-[90vh]">

      <!-- Header -->
      <div class="flex items-center justify-between px-5 py-4 border-b border-gray-200 dark:border-gray-700 flex-shrink-0">
        <h2 class="text-base font-semibold text-gray-900 dark:text-gray-100">
          {$editingContact ? 'Edit Contact' : 'New Contact'}
        </h2>
        <button on:click={close} class="text-gray-400 hover:text-gray-600 dark:hover:text-gray-200">
          <svg class="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M18 6 6 18M6 6l12 12"/></svg>
        </button>
      </div>

      <div class="overflow-y-auto flex-1 px-5 py-4 space-y-4">

        <!-- Name / job / org -->
        <p class={sectionCls}>Identity</p>
        <div>
          <label for="cm-name" class={labelCls}>Full name *</label>
          <input id="cm-name" type="text" bind:value={fullName} placeholder="Jane Smith" class="w-full {inputCls}" />
        </div>
        <div class="grid grid-cols-2 gap-2">
          <div>
            <label for="cm-job" class={labelCls}>Job title</label>
            <input id="cm-job" type="text" bind:value={jobTitle} placeholder="Software Engineer" class="w-full {inputCls}" />
          </div>
          <div>
            <label for="cm-org" class={labelCls}>Organisation</label>
            <input id="cm-org" type="text" bind:value={org} placeholder="Acme Corp" class="w-full {inputCls}" />
          </div>
        </div>

        <!-- Emails -->
        <p class={sectionCls}>Email</p>
        {#each emailList as entry, i}
          <div class="flex gap-2">
            <select bind:value={entry.type} class="w-24 flex-shrink-0 {inputCls}">
              <option value="work">Work</option>
              <option value="personal">Personal</option>
              <option value="other">Other</option>
            </select>
            <input type="email" bind:value={entry.address} placeholder="email@example.com"
              class="flex-1 {inputCls}" />
            {#if emailList.length > 1}
              <button on:click={() => removeEmail(i)}
                class="flex-shrink-0 text-gray-400 hover:text-red-500 px-1 transition-colors" title="Remove">
                <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M18 6 6 18M6 6l12 12"/></svg>
              </button>
            {/if}
          </div>
        {/each}
        <button on:click={addEmail}
          class="text-xs text-blue-600 dark:text-blue-400 hover:underline">+ Add email</button>

        <!-- Phones -->
        <p class={sectionCls}>Phone</p>
        {#each phoneList as entry, i}
          <div class="flex gap-2">
            <select bind:value={entry.type} class="w-24 flex-shrink-0 {inputCls}">
              <option value="mobile">Mobile</option>
              <option value="work">Work</option>
              <option value="home">Home</option>
              <option value="other">Other</option>
            </select>
            <input type="tel" bind:value={entry.number} placeholder="+1 555 0100"
              class="flex-1 {inputCls}" />
            {#if phoneList.length > 1}
              <button on:click={() => removePhone(i)}
                class="flex-shrink-0 text-gray-400 hover:text-red-500 px-1 transition-colors" title="Remove">
                <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M18 6 6 18M6 6l12 12"/></svg>
              </button>
            {/if}
          </div>
        {/each}
        <button on:click={addPhone}
          class="text-xs text-blue-600 dark:text-blue-400 hover:underline">+ Add phone</button>

        <!-- Address -->
        <p class={sectionCls}>Address</p>
        <div>
          <label for="cm-street" class={labelCls}>Street</label>
          <input id="cm-street" type="text" bind:value={street} placeholder="123 Main St" class="w-full {inputCls}" />
        </div>
        <div class="grid grid-cols-2 gap-2">
          <div>
            <label for="cm-city" class={labelCls}>City</label>
            <input id="cm-city" type="text" bind:value={city} placeholder="Springfield" class="w-full {inputCls}" />
          </div>
          <div>
            <label for="cm-state" class={labelCls}>State / Region</label>
            <input id="cm-state" type="text" bind:value={state} placeholder="IL" class="w-full {inputCls}" />
          </div>
        </div>
        <div class="grid grid-cols-2 gap-2">
          <div>
            <label for="cm-postal" class={labelCls}>Postal code</label>
            <input id="cm-postal" type="text" bind:value={postal} placeholder="62701" class="w-full {inputCls}" />
          </div>
          <div>
            <label for="cm-country" class={labelCls}>Country</label>
            <input id="cm-country" type="text" bind:value={country} placeholder="United States" class="w-full {inputCls}" />
          </div>
        </div>

        <!-- Other -->
        <p class={sectionCls}>Other</p>
        <div class="grid grid-cols-2 gap-2">
          <div>
            <label for="cm-bday" class={labelCls}>Birthday</label>
            <input id="cm-bday" type="date" bind:value={birthday} class="w-full {inputCls}" />
          </div>
          <div>
            <label for="cm-web" class={labelCls}>Website</label>
            <input id="cm-web" type="url" bind:value={website} placeholder="https://…" class="w-full {inputCls}" />
          </div>
        </div>
        <div>
          <label for="cm-notes" class={labelCls}>Notes</label>
          <textarea id="cm-notes" bind:value={notes} rows="3" placeholder="Notes…"
            class="w-full {inputCls} resize-none"></textarea>
        </div>

      </div>

      <!-- Footer -->
      <div class="flex items-center justify-between px-5 py-3 border-t border-gray-200 dark:border-gray-700 flex-shrink-0">
        <div>
          {#if $editingContact}
            <button on:click={remove} disabled={deleting}
              class="text-sm text-red-600 dark:text-red-400 hover:underline disabled:opacity-50">
              {deleting ? 'Deleting…' : 'Delete'}
            </button>
          {/if}
        </div>
        <div class="flex gap-2">
          <button on:click={close}
            class="px-4 py-1.5 text-sm rounded-lg border border-gray-200 dark:border-gray-600
                   text-gray-700 dark:text-gray-300 hover:bg-gray-50 dark:hover:bg-gray-700 transition-colors">
            Cancel
          </button>
          <button on:click={save} disabled={saving || !fullName.trim()}
            class="px-4 py-1.5 text-sm rounded-lg bg-blue-600 hover:bg-blue-700 text-white
                   disabled:opacity-50 transition-colors">
            {saving ? 'Saving…' : 'Save'}
          </button>
        </div>
      </div>

    </div>
  </div>
{/if}
