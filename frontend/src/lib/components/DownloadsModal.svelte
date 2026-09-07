<script>
  /**
   * Getting the desktop client.
   *
   * Only Linux is built here — Tauri links against the host's webview and
   * cannot be cross-compiled — so the other two platforms are offered as the
   * source archive plus the commands to build it. Saying that plainly beats a
   * greyed-out button labelled "coming soon".
   */
  import Modal from './Modal.svelte';
  import Spinner from './Spinner.svelte';
  import { downloadsOpen, appName } from '$lib/stores/mail.js';
  import { getDesktopReleases, downloadDesktopArtifact } from '$lib/api.js';
  import { formatBytes } from '$lib/fileTypes.js';
  import { toast } from '$lib/stores/toast.js';

  let artifacts = [];
  let loading = false;
  let error = '';
  let busyKey = null;
  let loadedFor = null;

  $: if ($downloadsOpen && loadedFor !== 'open') { loadedFor = 'open'; load(); }
  $: if (!$downloadsOpen) loadedFor = null;

  async function load() {
    loading = true;
    error = '';
    try {
      artifacts = await getDesktopReleases();
    } catch (e) {
      error = e?.message ?? 'Could not check for downloads.';
    } finally {
      loading = false;
    }
  }

  async function grab(key) {
    busyKey = key;
    try {
      await downloadDesktopArtifact(key);
    } catch (e) {
      toast(e?.message ?? 'Could not start that download.', 'error');
    } finally {
      busyKey = null;
    }
  }

  $: linux  = artifacts.find((a) => a.platform === 'linux');
  $: source = artifacts.find((a) => a.platform === 'source');

  const BUILD = [
    {
      os: 'Windows',
      needs: 'Rust, and Microsoft’s WebView2 runtime (already present on Windows 11).',
      commands: 'cd mecloud-desktop\\src-tauri\nrustup default stable\ncargo build --release',
      result: 'src-tauri\\target\\release\\mecloud-desktop.exe',
    },
    {
      os: 'macOS',
      needs: 'Rust and the Xcode command line tools (xcode-select --install).',
      commands: 'cd mecloud-desktop/src-tauri\nrustup default stable\ncargo build --release',
      result: 'src-tauri/target/release/mecloud-desktop',
    },
  ];

  const btn = `inline-flex items-center gap-2 px-3.5 py-1.5 rounded-lg text-sm font-medium
    bg-blue-500 hover:bg-blue-600 text-white disabled:opacity-60
    transition-colors duration-150 flex-shrink-0`;
</script>

<Modal
  open={$downloadsOpen}
  title="{$appName} for desktop"
  subtitle="The same apps in a native window, with a tray icon and mail-link handling"
  size="lg"
  on:close={() => downloadsOpen.set(false)}
>
  <div class="px-5 py-4">
    {#if loading}
      <div class="flex items-center justify-center gap-2 py-12 text-sm text-gray-500 dark:text-gray-400">
        <Spinner size="sm" label="" /> Checking what is available…
      </div>
    {:else if error}
      <div class="flex flex-col items-center gap-3 py-12">
        <p class="text-sm text-gray-500 dark:text-gray-400">{error}</p>
        <button on:click={load}
          class="text-xs font-medium px-3 py-1.5 rounded-lg bg-gray-100 dark:bg-gray-700
                 hover:bg-gray-200 dark:hover:bg-gray-600 text-gray-700 dark:text-gray-200">
          Try again
        </button>
      </div>
    {:else}
      <!-- Linux: built and served by this server -->
      {#if linux}
        <div class="flex items-start gap-3 p-4 rounded-xl border border-gray-200 dark:border-gray-700">
          <svg class="w-5 h-5 mt-0.5 flex-shrink-0 text-gray-500 dark:text-gray-400"
               viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6"
               stroke-linecap="round" stroke-linejoin="round">
            <rect x="3" y="4" width="18" height="12" rx="2" /><path d="M8 20h8M12 16v4" />
          </svg>
          <div class="flex-1 min-w-0">
            <p class="text-sm font-semibold text-gray-900 dark:text-gray-100">{linux.label}</p>
            <p class="text-xs text-gray-500 dark:text-gray-400 mt-0.5">{linux.description}</p>
            <p class="text-xs text-gray-400 dark:text-gray-500 mt-1">
              {formatBytes(linux.size)} · {linux.filename}
            </p>
          </div>
          <button class={btn} disabled={busyKey === linux.key} on:click={() => grab(linux.key)}>
            {#if busyKey === linux.key}
              <Spinner size="xs" label="" accent="border-t-white" cls="border-white/40" />
            {:else}
              <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                   stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
                <path d="M12 3v12m0 0-4-4m4 4 4-4M5 19h14" />
              </svg>
            {/if}
            Download
          </button>
        </div>
      {:else}
        <p class="text-sm text-gray-500 dark:text-gray-400 px-1 py-3">
          This server was built without the desktop client, so there is nothing to download here.
          The source is still on the project’s repository.
        </p>
      {/if}

      <!-- Windows and macOS: build from source -->
      <h3 class="text-xs font-semibold uppercase tracking-wider text-gray-400 dark:text-gray-500 mt-6 mb-2">
        Windows and macOS
      </h3>
      <p class="text-sm text-gray-600 dark:text-gray-300 mb-3">
        The client uses each system’s own webview, which cannot be cross-compiled,
        so those builds are not produced here. Download the source and build it on
        the machine you want to run it on — it takes a couple of minutes.
      </p>

      {#if source}
        <div class="flex items-center gap-3 p-3 rounded-xl border border-gray-200 dark:border-gray-700 mb-4">
          <svg class="w-5 h-5 flex-shrink-0 text-gray-500 dark:text-gray-400" viewBox="0 0 24 24"
               fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
            <path d="m10 12-2 2 2 2M14 12l2 2-2 2" />
            <path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" /><path d="M14 3v5h5" />
          </svg>
          <div class="flex-1 min-w-0">
            <p class="text-sm font-medium text-gray-800 dark:text-gray-200">{source.label}</p>
            <p class="text-xs text-gray-400 dark:text-gray-500">
              {formatBytes(source.size)} · {source.filename}
            </p>
          </div>
          <button class={btn} disabled={busyKey === source.key} on:click={() => grab(source.key)}>
            {#if busyKey === source.key}
              <Spinner size="xs" label="" accent="border-t-white" cls="border-white/40" />
            {/if}
            Download source
          </button>
        </div>
      {/if}

      <div class="space-y-4">
        {#each BUILD as target}
          <section>
            <h4 class="text-sm font-semibold text-gray-800 dark:text-gray-100">{target.os}</h4>
            <p class="text-xs text-gray-500 dark:text-gray-400 mt-0.5 mb-1.5">{target.needs}</p>
            <pre class="text-xs font-mono p-3 rounded-lg overflow-x-auto
                        bg-gray-50 dark:bg-gray-900/60 text-gray-700 dark:text-gray-300
                        border border-gray-200 dark:border-gray-700">{target.commands}</pre>
            <p class="text-xs text-gray-400 dark:text-gray-500 mt-1">
              Result: <code>{target.result}</code>
            </p>
          </section>
        {/each}
      </div>
    {/if}
  </div>

  <svelte:fragment slot="footer">
    <span class="text-xs text-gray-400 dark:text-gray-500">
      Download links are signed and expire after a few minutes.
    </span>
    <button on:click={() => downloadsOpen.set(false)}
      class="px-4 py-1.5 text-sm font-medium rounded-lg bg-blue-500 hover:bg-blue-600
             text-white transition-colors duration-150">
      Done
    </button>
  </svelte:fragment>
</Modal>
