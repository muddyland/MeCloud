<script>
  import { fly, fade } from 'svelte/transition';
  import { appPasswordsOpen, jmapAccountId, jmapSession } from '$lib/stores/mail.js';
  import { getAppPasswords, createAppPassword, deleteAppPassword } from '$lib/api.js';
  import { toast } from '$lib/stores/toast.js';

  let passwords = [];
  let loading   = false;
  let creating  = false;
  let newDesc   = '';
  let revealed  = null; // { id, secret } shown once after creation
  let copied    = false;

  async function load() {
    if (!$jmapAccountId || !$jmapSession) return;
    loading = true;
    try {
      passwords = await getAppPasswords($jmapAccountId, $jmapSession);
    } catch (e) {
      toast(e?.message ?? 'Failed to load app passwords', 'error');
    } finally {
      loading = false;
    }
  }

  async function create() {
    const desc = newDesc.trim();
    if (!desc) return;
    creating = true;
    try {
      const result = await createAppPassword($jmapAccountId, desc, $jmapSession);
      revealed = { id: result.id, secret: result.secret };
      copied   = false;
      newDesc  = '';
      await load();
    } catch (e) {
      toast(e?.message ?? 'Failed to create app password', 'error');
    } finally {
      creating = false;
    }
  }

  async function remove(id) {
    try {
      await deleteAppPassword($jmapAccountId, id, $jmapSession);
      if (revealed?.id === id) revealed = null;
      passwords = passwords.filter(p => p.id !== id);
      toast('App password deleted', 'success');
    } catch (e) {
      toast(e?.message ?? 'Failed to delete app password', 'error');
    }
  }

  async function copy() {
    await navigator.clipboard.writeText(revealed.secret);
    copied = true;
    setTimeout(() => { copied = false; }, 2000);
  }

  $: if ($appPasswordsOpen) { revealed = null; load(); }

  function close() { appPasswordsOpen.set(false); }
</script>

{#if $appPasswordsOpen}
  <!-- Backdrop -->
  <div class="fixed inset-0 z-50 bg-black/40 backdrop-blur-sm flex items-center justify-center p-4"
       transition:fade={{ duration: 150 }}
       on:click={close}
       aria-hidden="true">
  </div>

  <!-- Modal -->
  <div class="fixed inset-0 z-50 flex items-center justify-center p-4 pointer-events-none">
    <div
      transition:fly={{ y: 16, duration: 200 }}
      class="w-full max-w-lg pointer-events-auto rounded-xl shadow-2xl
             bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-700
             overflow-hidden"
      role="dialog"
      aria-label="App Passwords"
      on:click|stopPropagation
    >
      <!-- Header -->
      <div class="flex items-center justify-between px-5 py-4 border-b border-gray-200 dark:border-gray-700">
        <div>
          <h2 class="text-sm font-semibold text-gray-900 dark:text-gray-100">App Passwords</h2>
          <p class="text-xs text-gray-500 dark:text-gray-400 mt-0.5">
            Use these to sign in from email clients that don't support OAuth.
          </p>
        </div>
        <button on:click={close}
          class="text-gray-400 hover:text-gray-600 dark:hover:text-gray-200 text-xl leading-none ml-4">×</button>
      </div>

      <div class="px-5 py-4 space-y-4 max-h-[70vh] overflow-y-auto">

        <!-- Revealed password (shown once) -->
        {#if revealed}
          <div class="rounded-lg bg-green-50 dark:bg-green-900/20 border border-green-200 dark:border-green-700 p-4"
               transition:fly={{ y: -8, duration: 200 }}>
            <p class="text-xs font-medium text-green-800 dark:text-green-300 mb-2">
              Copy this password now — it won't be shown again.
            </p>
            <div class="flex items-center gap-2">
              <code class="flex-1 font-mono text-sm bg-white dark:bg-gray-900 border border-green-200 dark:border-green-700
                           rounded px-3 py-2 text-gray-800 dark:text-gray-100 select-all break-all">
                {revealed.secret}
              </code>
              <button on:click={copy}
                class="flex-shrink-0 px-3 py-2 text-xs font-medium rounded-md
                       {copied
                         ? 'bg-green-500 text-white'
                         : 'bg-green-100 dark:bg-green-800 text-green-700 dark:text-green-300 hover:bg-green-200 dark:hover:bg-green-700'}
                       transition-colors duration-150">
                {copied ? 'Copied!' : 'Copy'}
              </button>
            </div>
          </div>
        {/if}

        <!-- Create form -->
        <form on:submit|preventDefault={create} class="flex gap-2">
          <input
            bind:value={newDesc}
            placeholder="Label (e.g. Thunderbird, iPhone)"
            maxlength="100"
            class="flex-1 text-sm rounded-lg border border-gray-300 dark:border-gray-600
                   bg-white dark:bg-gray-700 text-gray-900 dark:text-gray-100
                   placeholder-gray-400 dark:placeholder-gray-500
                   px-3 py-2 focus:outline-none focus:ring-2 focus:ring-blue-500"
          />
          <button
            type="submit"
            disabled={creating || !newDesc.trim()}
            class="px-4 py-2 text-sm font-medium rounded-lg
                   bg-blue-600 hover:bg-blue-700 text-white
                   disabled:opacity-50 disabled:cursor-not-allowed
                   transition-colors duration-150 flex-shrink-0">
            {creating ? 'Creating…' : 'Create'}
          </button>
        </form>

        <!-- Password list -->
        {#if loading}
          <div class="space-y-2">
            {#each [1, 2] as _}
              <div class="h-12 rounded-lg bg-gray-100 dark:bg-gray-700 animate-pulse"></div>
            {/each}
          </div>
        {:else if passwords.length === 0}
          <p class="text-sm text-center text-gray-400 dark:text-gray-500 py-4">No app passwords yet.</p>
        {:else}
          <ul class="space-y-2">
            {#each passwords as pw (pw.id)}
              <li class="flex items-center justify-between gap-3 px-3 py-2.5 rounded-lg
                         bg-gray-50 dark:bg-gray-700/50 border border-gray-200 dark:border-gray-600">
                <div class="min-w-0">
                  <p class="text-sm font-medium text-gray-800 dark:text-gray-100 truncate">
                    {pw.description || '(unnamed)'}
                  </p>
                  {#if pw.expiresAt}
                    <p class="text-xs text-gray-400 dark:text-gray-500">
                      Expires {new Date(pw.expiresAt).toLocaleDateString()}
                    </p>
                  {/if}
                </div>
                <button
                  on:click={() => remove(pw.id)}
                  title="Delete"
                  class="flex-shrink-0 p-1.5 rounded-md text-gray-400 hover:text-red-500
                         hover:bg-red-50 dark:hover:bg-red-900/20 transition-colors duration-150">
                  <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                       stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
                    <polyline points="3 6 5 6 21 6"/>
                    <path d="M19 6l-1 14a2 2 0 01-2 2H8a2 2 0 01-2-2L5 6"/>
                    <path d="M10 11v6M14 11v6"/>
                    <path d="M9 6V4a1 1 0 011-1h4a1 1 0 011 1v2"/>
                  </svg>
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    </div>
  </div>
{/if}
