<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { provisioningStore } from "./lib/stores/provisioning.svelte";
  import { messagingStore } from "./lib/stores/messaging.svelte";
  import LinkDevice from "./lib/components/LinkDevice.svelte";
  import ChatLayout from "./lib/components/ChatLayout.svelte";
  import ToastContainer from "./lib/components/ToastContainer.svelte";

  let loading = $state(true);
  let teardown: (() => Promise<void>) | null = null;

  onMount(async () => {
    // Init IPC listeners ONCE per app lifetime. Returns a teardown handle so
    // HMR / app restart in dev doesn't pile up duplicate subscriptions.
    teardown = await messagingStore.initListeners();
    await provisioningStore.checkLinkStatus();
    loading = false;
  });

  onDestroy(() => {
    if (teardown) {
      void teardown();
      teardown = null;
    }
  });

  // Also track when provisioning completes
  let showChat = $derived(provisioningStore.isLinked);
</script>

{#if loading}
  <div class="loading">
    <div class="spinner"></div>
  </div>
{:else if showChat}
  <ChatLayout />
{:else}
  <LinkDevice />
{/if}

<ToastContainer />

<style>
  .loading {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 100%;
    height: 100vh;
    background: var(--bg-primary, #0f0f1a);
  }

  .spinner {
    width: 32px;
    height: 32px;
    border: 3px solid #27272a;
    border-top-color: #3b82f6;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
