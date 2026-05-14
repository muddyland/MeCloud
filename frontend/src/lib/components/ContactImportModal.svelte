<script>
  import {
    contactImportModalOpen, addressBooks, selectedAddressBook, contacts
  } from '$lib/stores/contacts.js';
  import { jmapAccountId, jmapSession } from '$lib/stores/mail.js';
  import { createContact } from '$lib/api.js';
  import { parseVCard } from '$lib/vcardParser.js';
  import { toast } from '$lib/stores/toast.js';

  let fileInput;
  let dragging  = false;
  let parsed    = [];
  let error     = '';
  let importing = false;
  let done      = 0;
  let targetBookId = null;

  $: targetBookId = $selectedAddressBook ?? $addressBooks[0]?.id ?? null;

  function close() {
    if (importing) return;
    contactImportModalOpen.set(false);
    parsed = [];
    error  = '';
    done   = 0;
  }

  async function readFile(f) {
    if (!f) return;
    if (!f.name.match(/\.vcf$/i) && f.type !== 'text/vcard') {
      error = 'Please select a .vcf file.';
      return;
    }
    error  = '';
    parsed = [];
    try {
      const text = await f.text();
      parsed = parseVCard(text);
      if (!parsed.length) error = 'No valid contacts found in this file.';
    } catch (e) {
      error = 'Could not read file: ' + (e?.message ?? 'unknown error');
    }
  }

  function onFileChange(e) { readFile(e.currentTarget.files?.[0]); }

  function onDrop(e) {
    dragging = false;
    e.preventDefault();
    readFile(e.dataTransfer?.files?.[0]);
  }

  async function importAll() {
    if (!parsed.length || importing) return;
    importing = true;
    done      = 0;
    let succeeded = 0;
    let failed    = 0;

    const bookId = targetBookId;

    for (const card of parsed) {
      try {
        const created = await createContact($jmapAccountId, $jmapSession, card, bookId);
        if (created) {
          contacts.update(list => [...list, { ...card, id: created.id }]);
          succeeded++;
        }
      } catch {
        failed++;
      }
      done++;
    }

    importing = false;

    if (failed === 0) {
      toast(`Imported ${succeeded} contact${succeeded !== 1 ? 's' : ''}`, 'success');
    } else {
      toast(`Imported ${succeeded}, failed ${failed}`, failed === parsed.length ? 'error' : 'success');
    }
    close();
  }

  const inputCls = 'text-sm px-3 py-2 rounded-lg border border-gray-200 dark:border-gray-600 bg-white dark:bg-gray-700 text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-blue-500';
</script>

{#if $contactImportModalOpen}
  <!-- svelte-ignore a11y-click-events-have-key-events -->
  <!-- svelte-ignore a11y-no-static-element-interactions -->
  <div class="fixed inset-0 bg-black/40 z-40 flex items-center justify-center p-4"
       on:click|self={close} role="dialog" aria-modal="true">

    <div class="w-full max-w-md bg-white dark:bg-gray-800 rounded-2xl shadow-xl z-50 flex flex-col">

      <!-- Header -->
      <div class="flex items-center justify-between px-5 py-4 border-b border-gray-200 dark:border-gray-700">
        <h2 class="text-base font-semibold text-gray-900 dark:text-gray-100">Import Contacts</h2>
        <button on:click={close} disabled={importing}
          class="text-gray-400 hover:text-gray-600 dark:hover:text-gray-200 disabled:opacity-40">
          <svg class="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M18 6 6 18M6 6l12 12"/>
          </svg>
        </button>
      </div>

      <div class="px-5 py-5 space-y-4">

        <!-- Drop zone / file picker -->
        <!-- svelte-ignore a11y-no-static-element-interactions -->
        <div
          class="relative border-2 border-dashed rounded-xl flex flex-col items-center justify-center
                 py-8 px-4 text-center cursor-pointer transition-colors
                 {dragging
                   ? 'border-blue-500 bg-blue-50 dark:bg-blue-900/20'
                   : 'border-gray-300 dark:border-gray-600 hover:border-blue-400 dark:hover:border-blue-500'}"
          on:dragover|preventDefault={() => (dragging = true)}
          on:dragleave={() => (dragging = false)}
          on:drop={onDrop}
          on:click={() => fileInput.click()}
          role="button"
          tabindex="0"
          on:keydown={(e) => e.key === 'Enter' && fileInput.click()}
        >
          <input bind:this={fileInput} type="file" accept=".vcf,text/vcard"
            on:change={onFileChange} class="hidden" />

          <svg class="w-8 h-8 text-gray-400 dark:text-gray-500 mb-2" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
            <path d="M12 16V4m0 0L8 8m4-4 4 4"/>
            <path d="M20 16.5A3.5 3.5 0 0 1 16.5 20h-9A3.5 3.5 0 0 1 4 16.5"/>
          </svg>
          <p class="text-sm text-gray-600 dark:text-gray-300">
            Drop a <span class="font-medium">.vcf</span> file here, or click to browse
          </p>
          <p class="text-xs text-gray-400 dark:text-gray-500 mt-1">vCard 2.1 · 3.0 · 4.0 supported</p>
        </div>

        {#if error}
          <p class="text-sm text-red-600 dark:text-red-400">{error}</p>
        {/if}

        {#if parsed.length > 0}
          <!-- Preview -->
          <div class="rounded-xl border border-gray-200 dark:border-gray-700 overflow-hidden">
            <div class="px-3 py-2 bg-gray-50 dark:bg-gray-700/50 text-xs font-medium text-gray-500 dark:text-gray-400 flex items-center justify-between">
              <span>{parsed.length} contact{parsed.length !== 1 ? 's' : ''} found</span>
            </div>
            <ul class="divide-y divide-gray-100 dark:divide-gray-700 max-h-40 overflow-y-auto">
              {#each parsed.slice(0, 8) as c}
                <li class="px-3 py-1.5 text-sm text-gray-800 dark:text-gray-200 truncate">
                  {c.name?.full ?? '—'}
                </li>
              {/each}
              {#if parsed.length > 8}
                <li class="px-3 py-1.5 text-xs text-gray-400 dark:text-gray-500 italic">
                  … and {parsed.length - 8} more
                </li>
              {/if}
            </ul>
          </div>

          <!-- Address book picker -->
          {#if $addressBooks.length > 1}
            <div>
              <label for="ci-book" class="block text-xs font-medium text-gray-500 dark:text-gray-400 mb-1">
                Import into address book
              </label>
              <select id="ci-book" bind:value={targetBookId} class="w-full {inputCls}">
                {#each $addressBooks as book}
                  <option value={book.id}>{book.name}</option>
                {/each}
              </select>
            </div>
          {/if}

          <!-- Progress bar (shown while importing) -->
          {#if importing}
            <div>
              <div class="flex justify-between text-xs text-gray-500 dark:text-gray-400 mb-1">
                <span>Importing…</span>
                <span>{done} / {parsed.length}</span>
              </div>
              <div class="w-full h-1.5 bg-gray-200 dark:bg-gray-700 rounded-full overflow-hidden">
                <div class="h-full bg-blue-500 rounded-full transition-all duration-150"
                     style="width: {parsed.length ? (done / parsed.length) * 100 : 0}%"></div>
              </div>
            </div>
          {/if}
        {/if}

      </div>

      <!-- Footer -->
      <div class="flex items-center justify-end gap-2 px-5 py-3 border-t border-gray-200 dark:border-gray-700">
        <button on:click={close} disabled={importing}
          class="px-4 py-1.5 text-sm rounded-lg border border-gray-200 dark:border-gray-600
                 text-gray-700 dark:text-gray-300 hover:bg-gray-50 dark:hover:bg-gray-700
                 disabled:opacity-50 transition-colors">
          Cancel
        </button>
        <button on:click={importAll} disabled={!parsed.length || importing}
          class="px-4 py-1.5 text-sm rounded-lg bg-blue-600 hover:bg-blue-700 text-white
                 disabled:opacity-50 transition-colors">
          {importing ? 'Importing…' : `Import ${parsed.length || ''}`}
        </button>
      </div>

    </div>
  </div>
{/if}
