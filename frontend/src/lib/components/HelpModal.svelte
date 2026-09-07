<script>
  /**
   * The documentation, as a modal.
   *
   * Sections down the side, topics in the body, and a search box that filters
   * both together — a help page that can only be scrolled is one nobody reads
   * past the first screen.
   */
  import Modal from './Modal.svelte';
  import { helpOpen, appName } from '$lib/stores/mail.js';
  import { HELP, searchHelp, countTopics } from '$lib/help.js';

  let query = '';
  let activeId = HELP[0].id;

  $: results = searchHelp(HELP, query);
  $: searching = query.trim().length > 0;
  $: total = countTopics(results);

  // While searching, every matching section is shown at once — hiding matches
  // behind a section the user has not clicked is the opposite of searching.
  $: sections = searching ? results : HELP.filter((s) => s.id === activeId);

  // A search that narrows past the selected section must not leave the
  // sidebar highlighting something with nothing in it.
  $: if (!searching && !HELP.some((s) => s.id === activeId)) activeId = HELP[0].id;

  function close() {
    helpOpen.set(false);
    query = '';
  }

  function pick(id) {
    query = '';
    activeId = id;
  }

  const ICONS = {
    home:     'M3 11.5 12 4l9 7.5M5.5 9.8V20h13V9.8',
    mail:     'M4 6a2 2 0 0 1 2-2h12a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2zM4 7l8 6 8-6',
    folder:   'M3 7a2 2 0 0 1 2-2h4l2 2.5h8a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z',
    note:     'M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8zM14 3v5h5M9 13h6M9 17h4',
    calendar: 'M4 6a2 2 0 0 1 2-2h12a2 2 0 0 1 2 2v13a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2zM4 10h16M8 3v4M16 3v4',
    contacts: 'M16 20v-1a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v1M10 11a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7M19 20v-1a4 4 0 0 0-3-3.9',
    shield:   'M12 3l7 3v5c0 4.5-3 8.3-7 10-4-1.7-7-5.5-7-10V6zM9 12l2 2 4-4',
  };

  const navCls = `flex items-center gap-2.5 w-full px-3 py-2 rounded-lg text-sm text-left
    transition-colors duration-100`;
</script>

<Modal
  open={$helpOpen}
  title="{$appName} help"
  subtitle="How the apps work, and where things live"
  size="xl"
  on:close={close}
>
  <div class="flex flex-col sm:flex-row min-h-[24rem]">

    <!-- Sections -->
    <aside class="sm:w-52 flex-shrink-0 p-3 sm:border-r border-b sm:border-b-0
                  border-gray-200 dark:border-gray-700
                  bg-gray-50/60 dark:bg-gray-900/40">
      <div class="relative flex items-center mb-3">
        <svg class="absolute left-2.5 w-3.5 h-3.5 text-gray-400 pointer-events-none"
             viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round">
          <circle cx="11" cy="11" r="8" /><path d="m21 21-4.35-4.35" />
        </svg>
        <input
          bind:value={query}
          data-autofocus
          placeholder="Search help…"
          class="w-full text-xs pl-8 pr-2 py-1.5 rounded-lg
                 bg-white dark:bg-gray-800 text-gray-800 dark:text-gray-200
                 placeholder-gray-400 dark:placeholder-gray-500
                 border border-gray-200 dark:border-gray-700
                 focus:border-blue-400 dark:focus:border-blue-500
                 focus:outline-none transition-colors duration-150"
        />
      </div>

      <nav class="flex sm:flex-col gap-1 overflow-x-auto sm:overflow-visible">
        {#each HELP as section (section.id)}
          {@const active = !searching && section.id === activeId}
          <button
            on:click={() => pick(section.id)}
            class="{navCls} flex-shrink-0
                   {active
                     ? 'bg-blue-100 dark:bg-blue-900/50 text-blue-700 dark:text-blue-300 font-medium'
                     : 'text-gray-600 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-gray-800'}"
          >
            <svg class="w-4 h-4 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
              <path d={ICONS[section.icon] ?? ICONS.note} />
            </svg>
            <span class="truncate">{section.title}</span>
          </button>
        {/each}
      </nav>
    </aside>

    <!-- Topics -->
    <div class="flex-1 min-w-0 px-5 py-4">
      {#if searching}
        <p class="text-xs text-gray-400 dark:text-gray-500 mb-4">
          {total === 0
            ? 'Nothing matches that.'
            : `${total} topic${total === 1 ? '' : 's'} matching “${query.trim()}”`}
        </p>
      {/if}

      {#if searching && total === 0}
        <div class="flex flex-col items-center gap-2 py-14 text-gray-400 dark:text-gray-500">
          <svg class="w-9 h-9 opacity-40" viewBox="0 0 24 24" fill="none" stroke="currentColor"
               stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="11" cy="11" r="8" /><path d="m21 21-4.35-4.35" />
          </svg>
          <p class="text-sm">Try a different word, or browse the sections.</p>
        </div>
      {/if}

      {#each sections as section (section.id)}
        {#if searching}
          <h3 class="text-xs font-semibold uppercase tracking-wider
                     text-gray-400 dark:text-gray-500 mt-6 first:mt-0 mb-3">
            {section.title}
          </h3>
        {/if}

        <div class="space-y-6">
          {#each section.topics as topic (topic.title)}
            <section>
              <h4 class="text-sm font-semibold text-gray-900 dark:text-gray-100 mb-1.5">
                {topic.title}
              </h4>
              {#each topic.body ?? [] as para}
                <p class="text-sm leading-relaxed text-gray-600 dark:text-gray-300 mb-2">
                  {para}
                </p>
              {/each}
              {#if topic.tip}
                <p class="flex items-start gap-2 mt-2 px-3 py-2 rounded-lg text-xs leading-relaxed
                          bg-blue-50 dark:bg-blue-900/20 text-blue-800 dark:text-blue-200">
                  <svg class="w-3.5 h-3.5 mt-0.5 flex-shrink-0" viewBox="0 0 24 24" fill="none"
                       stroke="currentColor" stroke-width="1.75" stroke-linecap="round"
                       stroke-linejoin="round">
                    <circle cx="12" cy="12" r="9" /><path d="M12 16v-4M12 8h.01" />
                  </svg>
                  {topic.tip}
                </p>
              {/if}
            </section>
          {/each}
        </div>
      {/each}
    </div>
  </div>

  <svelte:fragment slot="footer">
    <span class="text-xs text-gray-400 dark:text-gray-500">
      Press <kbd class="px-1 py-0.5 rounded bg-gray-100 dark:bg-gray-700 font-sans">?</kbd>
      for keyboard shortcuts
    </span>
    <button
      on:click={close}
      class="px-4 py-1.5 text-sm font-medium rounded-lg bg-blue-500 hover:bg-blue-600
             text-white transition-colors duration-150"
    >
      Done
    </button>
  </svelte:fragment>
</Modal>
