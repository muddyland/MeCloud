<script>
  /**
   * A read-only grid for one spreadsheet: sheet tabs along the bottom, like
   * Excel, column letters and row numbers, and every cell rendered as text.
   */
  import { columnLabel, MAX_ROWS, MAX_COLS } from '$lib/sheetPreview.js';

  /** @type {{ name: string, rows: string[][], truncated: boolean, hidden?: boolean }[]} */
  export let sheets = [];

  let active = 0;
  // Open on the first sheet a person would see, not a hidden helper sheet.
  $: if (sheets) active = Math.max(0, sheets.findIndex((s) => !s.hidden));
  $: sheet = sheets[active] ?? { name: '', rows: [], truncated: false };
  $: width = sheet.rows.reduce((w, r) => Math.max(w, r.length), 0);

  const NUMERIC = /^-?[\d.,]+%?$/;
</script>

<div class="w-full self-stretch h-[calc(90vh-9.5rem)] flex flex-col bg-white dark:bg-gray-800">
  <div class="flex-1 min-h-0 overflow-auto">
    {#if sheet.rows.length === 0}
      <p class="text-sm text-gray-500 dark:text-gray-400 py-12 text-center">This sheet is empty.</p>
    {:else}
      <table class="border-separate border-spacing-0 text-xs text-gray-800 dark:text-gray-200">
        <thead>
          <tr>
            <th class="sticky top-0 left-0 z-20 bg-gray-100 dark:bg-gray-700 border-r border-b border-gray-200 dark:border-gray-600 w-10"></th>
            {#each Array(width) as _, c}
              <th class="sticky top-0 z-10 bg-gray-100 dark:bg-gray-700 border-r border-b border-gray-200 dark:border-gray-600
                         px-2 py-1 font-medium text-gray-500 dark:text-gray-400 min-w-16">{columnLabel(c)}</th>
            {/each}
          </tr>
        </thead>
        <tbody>
          {#each sheet.rows as row, r}
            <tr>
              <th class="sticky left-0 z-10 bg-gray-100 dark:bg-gray-700 border-r border-b border-gray-200 dark:border-gray-600
                         px-2 py-1 font-medium text-gray-500 dark:text-gray-400 text-right">{r + 1}</th>
              {#each Array(width) as _, c}
                {@const value = row[c] ?? ''}
                <td class="border-r border-b border-gray-100 dark:border-gray-700 px-2 py-1 max-w-[24rem] truncate whitespace-pre
                           {NUMERIC.test(value) ? 'text-right tabular-nums' : ''}"
                    title={value.length > 40 ? value : undefined}>{value}</td>
              {/each}
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </div>

  {#if sheet.truncated}
    <p class="px-3 py-1.5 text-[11px] text-gray-500 dark:text-gray-400 border-t border-gray-200 dark:border-gray-700">
      Showing the first {MAX_ROWS.toLocaleString()} rows and {MAX_COLS} columns. Download the file for the rest.
    </p>
  {/if}

  {#if sheets.length > 1}
    <div class="flex gap-0.5 overflow-x-auto px-2 pt-1 border-t border-gray-200 dark:border-gray-700 bg-gray-50 dark:bg-gray-900/40"
         role="tablist" aria-label="Sheets">
      {#each sheets as s, i}
        <button
          role="tab"
          aria-selected={i === active}
          on:click={() => (active = i)}
          class="shrink-0 px-3 py-1.5 text-xs rounded-t-md border-b-2 transition-colors duration-150
                 {i === active
                   ? 'bg-white dark:bg-gray-800 border-blue-500 text-gray-900 dark:text-gray-100 font-medium'
                   : 'border-transparent text-gray-500 dark:text-gray-400 hover:text-gray-800 dark:hover:text-gray-200'}
                 {s.hidden ? 'italic' : ''}"
          title={s.hidden ? 'Hidden in the original' : undefined}
        >{s.name}</button>
      {/each}
    </div>
  {/if}
</div>
