<script>
  import { fly } from 'svelte/transition';
  import { sieveOpen, mailboxes } from '$lib/stores/mail.js';
  import { getSieveScript, saveSieveScript } from '$lib/api.js';

  const FIELDS = [
    { value: 'from',    label: 'From' },
    { value: 'to',      label: 'To' },
    { value: 'cc',      label: 'Cc' },
    { value: 'subject', label: 'Subject' },
  ];

  const OPS = [
    { value: 'contains',     label: 'contains' },
    { value: 'not_contains', label: 'does not contain' },
    { value: 'is',           label: 'is exactly' },
    { value: 'starts_with',  label: 'starts with' },
    { value: 'ends_with',    label: 'ends with' },
  ];

  const ACTION_TYPES = [
    { value: 'fileinto',    label: 'Move to folder' },
    { value: 'mark_read',   label: 'Mark as read' },
    { value: 'mark_unread', label: 'Mark as unread' },
    { value: 'discard',     label: 'Delete / Discard' },
    { value: 'stop',        label: 'Stop processing' },
  ];

  let scriptMeta     = null;
  let rules          = [];
  let loading        = true;
  let saving         = false;
  let saveError      = '';
  let supported      = true;
  let expandedId     = null;
  let hadCustomScript = false;

  // ── Load ─────────────────────────────────────────────────────────────────

  async function loadScript() {
    loading = true;
    rules = [];
    saveError = '';
    supported = true;
    hadCustomScript = false;
    scriptMeta = null;
    expandedId = null;

    try {
      const result = await getSieveScript();
      if (result === null) { supported = false; return; }
      scriptMeta = result;
      rules = extractRules(result.content ?? '');
    } catch {
      supported = false;
    } finally {
      loading = false;
    }
  }

  $: if ($sieveOpen) loadScript();

  function extractRules(content) {
    const m = content.match(/^# @rules ([A-Za-z0-9+/=%-]+)/m);
    if (!m) {
      if (content.trim()) hadCustomScript = true;
      return [];
    }
    try {
      return JSON.parse(decodeURIComponent(atob(m[1])));
    } catch {
      return [];
    }
  }

  // ── Rule helpers ──────────────────────────────────────────────────────────

  function newCondition() { return { field: 'from', op: 'contains', value: '' }; }
  function newAction()    { return { type: 'fileinto', mailboxId: '', mailboxName: '' }; }

  function addRule() {
    const id = crypto.randomUUID();
    rules = [...rules, {
      id,
      name: 'New Rule',
      enabled: true,
      match: 'all',
      conditions: [newCondition()],
      actions: [newAction()],
    }];
    expandedId = id;
  }

  function removeRule(id) {
    rules = rules.filter(r => r.id !== id);
    if (expandedId === id) expandedId = null;
  }

  function toggleExpand(id) { expandedId = expandedId === id ? null : id; }

  function addCondition(rule)          { rule.conditions = [...rule.conditions, newCondition()]; rules = rules; }
  function removeCondition(rule, i)    { rule.conditions = rule.conditions.filter((_, j) => j !== i); rules = rules; }
  function addAction(rule)             { rule.actions = [...rule.actions, newAction()]; rules = rules; }
  function removeAction(rule, i)       { rule.actions = rule.actions.filter((_, j) => j !== i); rules = rules; }

  function onActionTypeChange(action) {
    if (action.type !== 'fileinto') { action.mailboxId = ''; action.mailboxName = ''; }
    rules = rules;
  }

  function onMailboxChange(action, mbId) {
    action.mailboxId  = mbId;
    action.mailboxName = $mailboxes.find(m => m.id === mbId)?.name ?? '';
    rules = rules;
  }

  // ── Sieve generation ──────────────────────────────────────────────────────

  function esc(s) { return s.replace(/\\/g, '\\\\').replace(/"/g, '\\"'); }

  function buildCondition(c) {
    const h = { from: 'From', to: 'To', cc: 'Cc', subject: 'Subject' }[c.field] ?? c.field;
    switch (c.op) {
      case 'contains':     return `header :contains "${h}" "${esc(c.value)}"`;
      case 'not_contains': return `not header :contains "${h}" "${esc(c.value)}"`;
      case 'is':           return `header :is "${h}" "${esc(c.value)}"`;
      case 'starts_with':  return `header :matches "${h}" "${esc(c.value)}*"`;
      case 'ends_with':    return `header :matches "${h}" "*${esc(c.value)}"`;
      default:             return `header :contains "${h}" "${esc(c.value)}"`;
    }
  }

  function generateSieve(ruleList) {
    const active = ruleList.filter(r =>
      r.enabled && r.conditions.length && r.conditions.every(c => c.value.trim()) &&
      r.actions.length && r.actions.every(a => a.type !== 'fileinto' || a.mailboxName)
    );

    const needsFlags = active.some(r =>
      r.actions.some(a => a.type === 'mark_read' || a.type === 'mark_unread')
    );

    const reqs = ['fileinto'];
    if (needsFlags) reqs.push('imap4flags');

    const lines = [];
    lines.push(`# @rules ${btoa(encodeURIComponent(JSON.stringify(ruleList)))}`);
    lines.push(`require [${reqs.map(r => `"${r}"`).join(', ')}];`);
    lines.push('');

    for (const rule of active) {
      lines.push(`# Rule: "${esc(rule.name)}"`);
      const parts = rule.conditions.map(buildCondition);
      const ifExpr = parts.length === 1
        ? parts[0]
        : `${rule.match === 'any' ? 'anyof' : 'allof'} (\n    ${parts.join(',\n    ')}\n  )`;
      lines.push(`if ${ifExpr} {`);
      for (const a of rule.actions) {
        if      (a.type === 'fileinto'    && a.mailboxName) lines.push(`  fileinto "${esc(a.mailboxName)}";`);
        else if (a.type === 'mark_read')                    lines.push(`  addflag "\\\\Seen";`);
        else if (a.type === 'mark_unread')                  lines.push(`  removeflag "\\\\Seen";`);
        else if (a.type === 'discard')                      lines.push(`  discard;`);
        else if (a.type === 'stop')                         lines.push(`  stop;`);
      }
      lines.push('}');
      lines.push('');
    }

    return lines.join('\n');
  }

  // ── Save ──────────────────────────────────────────────────────────────────

  async function save() {
    saving = true;
    saveError = '';
    try {
      const content = generateSieve(rules);
      const result  = await saveSieveScript(
        scriptMeta?.id ?? null,
        scriptMeta?.name ?? 'My Rules',
        content,
        true
      );
      if (scriptMeta) {
        scriptMeta = { ...scriptMeta, id: result.id };
      } else {
        scriptMeta = { id: result.id, name: result.name, content, isActive: true };
      }
      sieveOpen.set(false);
    } catch (e) {
      saveError = e?.message ?? 'Failed to save. Please try again.';
    } finally {
      saving = false;
    }
  }

  const selectCls = `text-xs rounded-md border border-gray-200 dark:border-gray-600
    bg-white dark:bg-gray-700 text-gray-700 dark:text-gray-300
    px-2 py-1 focus:outline-none focus:ring-1 focus:ring-blue-500`;
</script>

{#if $sieveOpen}
  <div class="fixed inset-0 z-50 flex items-center justify-center bg-black/40 dark:bg-black/60 p-4">
    <div
      class="w-full max-w-2xl flex flex-col rounded-xl shadow-2xl bg-white dark:bg-gray-800
             ring-1 ring-black/5 dark:ring-white/10 overflow-hidden max-h-[90vh]"
      transition:fly={{ y: 12, duration: 200 }}
    >

      <!-- Header -->
      <div class="px-5 py-4 border-b border-gray-200 dark:border-gray-700 flex items-center justify-between flex-shrink-0">
        <div>
          <p class="text-sm font-semibold text-gray-800 dark:text-gray-100">Filters / Rules</p>
          <p class="text-xs text-gray-500 dark:text-gray-400 mt-0.5">Automatically sort, label, or discard incoming messages</p>
        </div>
        <button
          on:click={() => sieveOpen.set(false)}
          class="text-gray-400 hover:text-gray-600 dark:hover:text-gray-200 text-xl leading-none p-1"
        >&times;</button>
      </div>

      <!-- Body -->
      <div class="flex-1 overflow-y-auto min-h-0 p-4 space-y-3">

        {#if !supported}
          <div class="flex flex-col items-center justify-center py-16 gap-3 text-gray-400 dark:text-gray-500">
            <svg class="w-10 h-10 opacity-50" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126zM12 15.75h.007v.008H12v-.008z"/>
            </svg>
            <p class="text-sm">Your server does not support Sieve rules.</p>
          </div>

        {:else if loading}
          <div class="space-y-2">
            {#each [1, 2] as _}
              <div class="h-11 rounded-lg bg-gray-100 dark:bg-gray-700 animate-pulse"></div>
            {/each}
          </div>

        {:else}
          {#if hadCustomScript}
            <div class="px-3 py-2 rounded-lg bg-amber-50 dark:bg-amber-900/20
                        border border-amber-200 dark:border-amber-700/50
                        text-xs text-amber-700 dark:text-amber-400">
              An existing custom Sieve script was found but cannot be converted to rules.
              Saving will replace it with the rules below.
            </div>
          {/if}

          {#if rules.length === 0}
            <div class="flex flex-col items-center justify-center py-12 gap-2 text-gray-400 dark:text-gray-500">
              <span class="text-4xl">🔍</span>
              <p class="text-sm font-medium text-gray-600 dark:text-gray-300">No rules yet</p>
              <p class="text-xs">Add a rule to automatically organise your inbox</p>
            </div>
          {/if}

          {#each rules as rule (rule.id)}
            {@const expanded = expandedId === rule.id}
            <div
              class="rounded-lg border overflow-hidden transition-colors duration-150
                     {expanded
                       ? 'border-blue-200 dark:border-blue-700/60 shadow-sm'
                       : 'border-gray-200 dark:border-gray-700'}"
              in:fly={{ y: 4, duration: 150 }}
            >
              <!-- Rule title row -->
              <div class="flex items-center gap-2 px-3 py-2.5 bg-white dark:bg-gray-800">

                <button
                  on:click={() => toggleExpand(rule.id)}
                  class="text-gray-400 hover:text-gray-600 dark:hover:text-gray-300
                         flex-shrink-0 text-xs w-4 text-center leading-none"
                >{expanded ? '▼' : '▶'}</button>

                {#if expanded}
                  <input
                    bind:value={rule.name}
                    class="flex-1 text-sm font-medium text-gray-800 dark:text-gray-100 bg-transparent
                           border-b border-gray-300 dark:border-gray-600 py-0.5
                           focus:outline-none focus:border-blue-500 min-w-0"
                  />
                {:else}
                  <button
                    on:click={() => toggleExpand(rule.id)}
                    class="flex-1 text-left text-sm font-medium text-gray-800 dark:text-gray-100 truncate"
                  >{rule.name || 'Untitled Rule'}</button>
                {/if}

                <!-- Enable toggle -->
                <button
                  on:click={() => { rule.enabled = !rule.enabled; rules = rules; }}
                  class="relative overflow-hidden flex-shrink-0 w-8 h-4 rounded-full transition-colors duration-200
                         {rule.enabled ? 'bg-blue-500' : 'bg-gray-300 dark:bg-gray-600'}"
                  title={rule.enabled ? 'Enabled — click to disable' : 'Disabled — click to enable'}
                >
                  <span class="absolute top-0.5 w-3 h-3 rounded-full bg-white shadow-sm transition-all duration-200"
                               style="left: {rule.enabled ? '18px' : '2px'}"></span>
                </button>
              </div>

              <!-- Expanded body -->
              {#if expanded}
                <div class="px-4 pt-3 pb-4 border-t border-gray-100 dark:border-gray-700/50 space-y-4
                            bg-gray-50/50 dark:bg-gray-800/30">

                  <!-- Conditions -->
                  <div>
                    <div class="flex items-center gap-1.5 mb-2">
                      <span class="text-xs text-gray-500 dark:text-gray-400">Match</span>
                      <select bind:value={rule.match} class="{selectCls}">
                        <option value="all">all</option>
                        <option value="any">any</option>
                      </select>
                      <span class="text-xs text-gray-500 dark:text-gray-400">of these conditions:</span>
                    </div>

                    <div class="space-y-1.5">
                      {#each rule.conditions as cond, ci}
                        <div class="flex items-center gap-1.5">
                          <select bind:value={cond.field} class="{selectCls} w-24 flex-shrink-0">
                            {#each FIELDS as f}
                              <option value={f.value}>{f.label}</option>
                            {/each}
                          </select>

                          <select bind:value={cond.op} class="{selectCls} flex-shrink-0">
                            {#each OPS as o}
                              <option value={o.value}>{o.label}</option>
                            {/each}
                          </select>

                          <input
                            bind:value={cond.value}
                            placeholder="value…"
                            class="flex-1 min-w-0 text-xs rounded-md border border-gray-200 dark:border-gray-600
                                   bg-white dark:bg-gray-700 text-gray-800 dark:text-gray-100
                                   px-2 py-1 focus:outline-none focus:ring-1 focus:ring-blue-500"
                          />

                          <button
                            on:click={() => removeCondition(rule, ci)}
                            disabled={rule.conditions.length === 1}
                            class="flex-shrink-0 text-xs text-gray-300 dark:text-gray-600
                                   hover:text-red-400 dark:hover:text-red-400
                                   disabled:opacity-30 disabled:cursor-not-allowed
                                   transition-colors duration-100"
                          >✕</button>
                        </div>
                      {/each}
                    </div>

                    <button
                      on:click={() => addCondition(rule)}
                      class="mt-2 text-xs text-blue-500 hover:text-blue-600 dark:hover:text-blue-400
                             transition-colors duration-100"
                    >+ Add condition</button>
                  </div>

                  <!-- Divider -->
                  <div class="border-t border-gray-200 dark:border-gray-700/50"></div>

                  <!-- Actions -->
                  <div>
                    <p class="text-xs text-gray-500 dark:text-gray-400 mb-2">Perform these actions:</p>

                    <div class="space-y-1.5">
                      {#each rule.actions as action, ai}
                        <div class="flex items-center gap-1.5">
                          <select
                            bind:value={action.type}
                            on:change={() => onActionTypeChange(action)}
                            class="{selectCls} flex-shrink-0"
                          >
                            {#each ACTION_TYPES as t}
                              <option value={t.value}>{t.label}</option>
                            {/each}
                          </select>

                          {#if action.type === 'fileinto'}
                            <select
                              value={action.mailboxId}
                              on:change={(e) => onMailboxChange(action, e.currentTarget.value)}
                              class="{selectCls} flex-1 min-w-0"
                            >
                              <option value="">Select folder…</option>
                              {#each $mailboxes as mb}
                                <option value={mb.id}>{mb.name}</option>
                              {/each}
                            </select>
                          {:else}
                            <div class="flex-1"></div>
                          {/if}

                          <button
                            on:click={() => removeAction(rule, ai)}
                            disabled={rule.actions.length === 1}
                            class="flex-shrink-0 text-xs text-gray-300 dark:text-gray-600
                                   hover:text-red-400 dark:hover:text-red-400
                                   disabled:opacity-30 disabled:cursor-not-allowed
                                   transition-colors duration-100"
                          >✕</button>
                        </div>
                      {/each}
                    </div>

                    <button
                      on:click={() => addAction(rule)}
                      class="mt-2 text-xs text-blue-500 hover:text-blue-600 dark:hover:text-blue-400
                             transition-colors duration-100"
                    >+ Add action</button>
                  </div>

                  <!-- Delete rule -->
                  <div class="flex justify-end pt-1">
                    <button
                      on:click={() => removeRule(rule.id)}
                      class="text-xs text-gray-400 dark:text-gray-500
                             hover:text-red-500 dark:hover:text-red-400
                             transition-colors duration-100"
                    >Delete rule</button>
                  </div>
                </div>
              {/if}
            </div>
          {/each}

          <button
            on:click={addRule}
            class="w-full flex items-center justify-center gap-2 px-4 py-3 rounded-lg
                   border-2 border-dashed border-gray-200 dark:border-gray-700
                   text-sm text-gray-500 dark:text-gray-400
                   hover:border-blue-300 dark:hover:border-blue-600
                   hover:text-blue-500 dark:hover:text-blue-400
                   transition-colors duration-150"
          >+ Add Rule</button>
        {/if}
      </div>

      <!-- Footer -->
      {#if supported && !loading}
        <div class="px-5 py-3 border-t border-gray-200 dark:border-gray-700 flex items-center justify-between flex-shrink-0">
          <div>
            {#if saveError}
              <p class="text-xs text-red-500">{saveError}</p>
            {/if}
          </div>
          <div class="flex items-center gap-3">
            <button
              on:click={() => sieveOpen.set(false)}
              class="text-sm text-gray-500 hover:text-gray-700 dark:hover:text-gray-300
                     transition-colors duration-100"
            >Cancel</button>
            <button
              on:click={save}
              disabled={saving}
              class="text-sm font-medium text-white bg-blue-500 hover:bg-blue-600 px-4 py-1.5 rounded-lg
                     disabled:opacity-50 disabled:cursor-not-allowed transition-colors duration-150"
            >{saving ? 'Saving…' : 'Save Rules'}</button>
          </div>
        </div>
      {/if}

    </div>
  </div>
{/if}
