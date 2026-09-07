<script>
  import '../app.css';
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import { darkMode, appName, inboxUnread } from '$lib/stores/mail.js';
  import { getMe, getAppConfig } from '$lib/api.js';
  import ProgressBar from '$lib/components/ProgressBar.svelte';
  import Spinner from '$lib/components/Spinner.svelte';
  import BrandMark from '$lib/components/BrandMark.svelte';
  import CommandPalette from '$lib/components/CommandPalette.svelte';
  import HelpModal from '$lib/components/HelpModal.svelte';
  import DownloadsModal from '$lib/components/DownloadsModal.svelte';
  import ShortcutsHelp from '$lib/components/ShortcutsHelp.svelte';

  let checkingAuth = true;

  // Badge the tab the way every other mail client does, so an unread count is
  // visible without switching to the app.
  $: title = $inboxUnread > 0 ? `(${$inboxUnread}) ${$appName}` : $appName;

  onMount(async () => {
    darkMode.init($darkMode);

    const isAuthRoute = $page.url.pathname.startsWith('/auth');

    try {
      const [config, me] = await Promise.all([
        getAppConfig(),
        isAuthRoute ? Promise.resolve({ authenticated: true }) : getMe(),
      ]);

      if (config?.appName) appName.set(config.appName);

      if (!isAuthRoute && !me.authenticated) {
        goto('/auth/login');
        return;
      }
    } catch (e) {
      // Never leave the user staring at a spinner because a probe failed —
      // render the app and let the individual views report their own errors.
      console.error('Startup check failed:', e);
    } finally {
      checkingAuth = false;
    }
  });
</script>

<svelte:head>
  <title>{title}</title>
</svelte:head>

<!-- Thin activity bar: any in-flight request drives it, so the app always shows
     it is doing something even when the work has no local spinner. -->
<ProgressBar />

{#if checkingAuth}
  <div class="fixed inset-0 flex flex-col items-center justify-center gap-3
              bg-gray-100 dark:bg-gray-950">
    <BrandMark cls="w-10 h-10 text-blue-500 dark:text-blue-400" />
    <Spinner size="md" label="Signing in" />
    <p class="text-sm text-gray-500 dark:text-gray-400">Opening your account…</p>
  </div>
{:else}
  <slot />
  <!-- Global, so Ctrl/Cmd+K works from every route -->
  <CommandPalette />
  <!-- Likewise: the navbar carries help and shortcuts on every page, so the
       dialogs they open have to exist on every page too. -->
  <HelpModal />
  <DownloadsModal />
  <ShortcutsHelp />
{/if}
