<script>
  /**
   * Add and remove tags on one item or a whole selection.
   *
   * Nothing is written until Save, and what is sent is the *difference* —
   * tags added and tags removed — rather than the final list. Another device
   * may have tagged the same file while this dialog was open, and a diff
   * merges with that where a replacement would silently undo it.
   *
   * With several items selected, a tag on all of them is shown plainly and a
   * tag on only some of them is shown with its count; removing either removes
   * it from every selected item, and "add to all" fills in the rest.
   */
  import { createEventDispatcher } from 'svelte';
  import Modal from './Modal.svelte';
  import Spinner from './Spinner.svelte';
  import {
    normalizeTag, canonicalize, splitTagInput, tagKey, tagsFor,
    MAX_TAG_LENGTH, MAX_TAGS_PER_FILE,
  } from '#lib/tags.js';

  export let open = false;
  /** The items being tagged. */
  export let nodes = [];
  /** id → tags, the whole index. */
  export let tags = new Map();
  /** Every tag spelling in use, offered as suggestions in this order. */
  export let known = [];
  export let busy = false;
  /** A save failure, or why tags are read-only. */
  export let error = '';
  export let readOnly = false;

  const dispatch = createEventDispatcher();

  /** @type {{tag: string, key: string, count: number, all: boolean}[]} */
  let entries = [];
  let originalKeys = new Set();
  /** Keys that were on every item when the dialog opened: nothing to add. */
  let originalAll = new Set();
  let input = '';
  let inputError = '';
  let inputEl;
  let openedFor = '';

  // Reset whenever the dialog opens for a (possibly different) set of items.
  $: signature = open ? nodes.map((n) => n.id).join('\n') : '';
  $: if (open && signature !== openedFor) reset();
  $: if (!open) openedFor = '';

  function reset() {
    openedFor = signature;
    const counts = new Map();
    for (const node of nodes) {
      for (const tag of tagsFor(tags, node.id)) {
        const key = tagKey(tag);
        const entry = counts.get(key);
        if (entry) entry.count += 1;
        else counts.set(key, { tag, key, count: 1, all: false });
      }
    }
    entries = [...counts.values()].map((e) => ({ ...e, all: e.count === nodes.length }));
    originalKeys = new Set(entries.map((e) => e.key));
    originalAll = new Set(entries.filter((e) => e.all).map((e) => e.key));
    input = '';
    inputError = '';
  }

  $: total = nodes.length;
  $: bulk = total > 1;
  $: title = bulk ? `Tags for ${total} items` : `Tags for “${nodes[0]?.name ?? ''}”`;

  $: query = tagKey(normalizeTag(input) ?? '');
  $: present = new Set(entries.filter((e) => e.all).map((e) => e.key));
  $: suggestions = known
    .filter((t) => !present.has(tagKey(t)) && (!query || tagKey(t).includes(query)))
    .slice(0, 8);

  /** Add typed (or picked) tags to every item. */
  function commit(text) {
    inputError = '';
    const parts = splitTagInput(text).filter((p) => p.trim());
    for (const part of parts) {
      const tag = normalizeTag(part);
      if (!tag) {
        inputError = [...part.trim().replace(/^#+/, '').trim()].length > MAX_TAG_LENGTH
          ? `A tag can be at most ${MAX_TAG_LENGTH} characters.`
          : `“${part.trim()}” cannot be used as a tag.`;
        continue;
      }
      // The spelling already in use wins, so "tax" joins an existing "Tax".
      const [spelled] = canonicalize([tag], [...entries.map((e) => e.tag), ...known]);
      const key = tagKey(spelled);
      const existing = entries.find((e) => e.key === key);
      if (existing) {
        entries = entries.map((e) => (e.key === key ? { ...e, all: true, count: total } : e));
        continue;
      }
      if (entries.length >= MAX_TAGS_PER_FILE) {
        inputError = `An item can have at most ${MAX_TAGS_PER_FILE} tags.`;
        break;
      }
      entries = [...entries, { tag: spelled, key, count: total, all: true }];
    }
  }

  function onInput(event) {
    const field = event.currentTarget;
    // A comma finishes a tag, the way it does in an email address field.
    if (field.value.includes(',')) {
      const parts = splitTagInput(field.value);
      input = parts.pop() ?? '';
      commit(parts.join(','));
      // Set directly too: if `input` did not change (a lone comma), Svelte
      // would leave the comma showing in the field.
      field.value = input;
    } else {
      input = field.value;
    }
  }

  function onKeydown(event) {
    if (event.key !== 'Enter') return;
    event.preventDefault();
    // Enter on an empty field saves, so the whole dialog works from the keyboard.
    if (!input.trim()) { save(); return; }
    commit(input);
    // A tag that was refused stays in the field, next to the reason, to fix.
    if (!inputError) input = '';
  }

  function pick(tag) {
    commit(tag);
    input = '';
    inputEl?.focus();
  }

  function removeEntry(key) {
    entries = entries.filter((e) => e.key !== key);
  }

  function addToAll(key) {
    entries = entries.map((e) => (e.key === key ? { ...e, all: true, count: total } : e));
  }

  /** What Save sends: tags newly on every item, and tags taken off. */
  function computeChange(list) {
    const keep = new Set(list.map((e) => e.key));
    return {
      add: list.filter((e) => e.all && !originalAll.has(e.key)).map((e) => e.tag),
      remove: [...originalKeys].filter((k) => !keep.has(k)),
    };
  }

  $: change = computeChange(entries);
  $: dirty = change.add.length > 0 || change.remove.length > 0 || input.trim() !== '';

  function save() {
    if (busy || readOnly) return;
    if (input.trim()) {
      commit(input);
      if (inputError) return;
      input = '';
    }
    // Computed here rather than read from `change`: the commit above has not
    // been through a reactive update yet.
    const next = computeChange(entries);
    if (!next.add.length && !next.remove.length) { dispatch('close'); return; }
    dispatch('save', next);
  }

  const chipCls =
    'inline-flex items-center gap-1 max-w-full pl-2 pr-1 py-0.5 rounded-full text-xs';
</script>

<Modal {open} {title} subtitle={bulk ? 'Changes apply to every selected item' : ''} size="sm" {busy}
       on:close={() => dispatch('close')}>
  <div class="px-5 py-4 space-y-3">
    {#if readOnly}
      <p class="text-xs text-amber-700 dark:text-amber-300">{error}</p>
    {/if}

    <!-- Current tags -->
    <div>
      <p id="tags-current" class="text-xs font-medium text-gray-500 dark:text-gray-400 mb-1.5">
        {bulk ? 'On these items' : 'Tags'}
      </p>
      {#if entries.length}
        <ul class="flex flex-wrap gap-1.5" aria-labelledby="tags-current">
          {#each entries as entry (entry.key)}
            <li class="{chipCls}
                       {entry.all
                         ? 'bg-blue-50 dark:bg-blue-900/40 text-blue-700 dark:text-blue-300'
                         : 'bg-gray-100 dark:bg-gray-700 text-gray-600 dark:text-gray-300 border border-dashed border-gray-300 dark:border-gray-600'}">
              <span class="truncate">{entry.tag}</span>
              {#if !entry.all}
                <span class="text-[11px] text-gray-400 dark:text-gray-500 tabular-nums">{entry.count}/{total}</span>
                <button type="button" on:click={() => addToAll(entry.key)} disabled={readOnly || busy}
                        aria-label="Add “{entry.tag}” to all selected items" title="Add to all"
                        class="p-0.5 rounded-full hover:bg-gray-200 dark:hover:bg-gray-600 disabled:opacity-40">
                  <svg class="w-3 h-3" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                       stroke-width="2.5" stroke-linecap="round"><path d="M12 5v14M5 12h14" /></svg>
                </button>
              {/if}
              <button type="button" on:click={() => removeEntry(entry.key)} disabled={readOnly || busy}
                      aria-label={bulk ? `Remove “${entry.tag}” from all selected items` : `Remove “${entry.tag}”`}
                      title={bulk ? 'Remove from all' : 'Remove'}
                      class="p-0.5 rounded-full hover:bg-blue-100 dark:hover:bg-blue-800/60 disabled:opacity-40">
                <svg class="w-3 h-3" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                     stroke-width="2.5" stroke-linecap="round"><path d="M18 6 6 18M6 6l12 12" /></svg>
              </button>
            </li>
          {/each}
        </ul>
      {:else}
        <p class="text-xs text-gray-400 dark:text-gray-500">No tags yet.</p>
      {/if}
    </div>

    <!-- Add -->
    <label class="block">
      <span class="block text-xs font-medium text-gray-500 dark:text-gray-400 mb-1.5">
        Add {bulk ? 'to all' : 'tags'}
      </span>
      <input
        bind:this={inputEl}
        type="text"
        value={input}
        on:input={onInput}
        on:keydown={onKeydown}
        disabled={readOnly || busy}
        placeholder="Type a tag, then Enter or comma"
        autocomplete="off"
        data-autofocus
        aria-describedby="tags-hint"
        class="w-full rounded-lg border border-gray-200 dark:border-gray-600
               bg-white dark:bg-gray-700 text-sm text-gray-800 dark:text-gray-100
               px-3 py-2 focus:outline-hidden focus:ring-2 focus:ring-blue-500
               disabled:opacity-60 disabled:cursor-not-allowed"
      />
    </label>
    <p id="tags-hint" class="text-[11px] text-gray-400 dark:text-gray-500 -mt-1.5">
      Up to {MAX_TAGS_PER_FILE} tags per item, {MAX_TAG_LENGTH} characters each.
    </p>

    {#if inputError}
      <p class="text-xs text-red-500" role="alert">{inputError}</p>
    {/if}

    {#if suggestions.length && !readOnly}
      <div>
        <p id="tags-suggested" class="text-xs font-medium text-gray-500 dark:text-gray-400 mb-1.5">
          {query ? 'Matching tags' : 'Your tags'}
        </p>
        <div class="flex flex-wrap gap-1.5" role="group" aria-labelledby="tags-suggested">
          {#each suggestions as tag (tag)}
            <button type="button" on:click={() => pick(tag)} disabled={busy}
                    class="max-w-full truncate px-2 py-0.5 rounded-full text-xs
                           border border-gray-200 dark:border-gray-600
                           text-gray-600 dark:text-gray-300
                           hover:bg-gray-100 dark:hover:bg-gray-700
                           focus:outline-hidden focus-visible:ring-2 focus-visible:ring-blue-500
                           transition-colors duration-100">
              + {tag}
            </button>
          {/each}
        </div>
      </div>
    {/if}

    {#if error && !readOnly}
      <p class="text-xs text-red-500" role="alert">{error}</p>
    {/if}
  </div>

  <svelte:fragment slot="footer">
    <span></span>
    <div class="flex items-center gap-2">
      <button
        on:click={() => dispatch('close')}
        disabled={busy}
        class="text-sm text-gray-500 hover:text-gray-700 dark:hover:text-gray-300
               px-3 py-1.5 rounded-lg hover:bg-gray-100 dark:hover:bg-gray-700
               disabled:opacity-40 transition-colors duration-150"
      >
        Cancel
      </button>
      <button
        on:click={save}
        disabled={busy || readOnly || !dirty}
        class="inline-flex items-center gap-2 text-sm font-medium text-white
               bg-blue-500 hover:bg-blue-600 px-3 py-1.5 rounded-lg
               disabled:opacity-50 disabled:cursor-not-allowed
               transition-colors duration-150"
      >
        {#if busy}
          <Spinner size="xs" label="" accent="border-t-white" cls="border-white/40" />
          Saving…
        {:else}
          Save
        {/if}
      </button>
    </div>
  </svelte:fragment>
</Modal>
