<script lang="ts">
  import { onMount } from "svelte";
  import { provisioningStore } from "./lib/stores/provisioning.svelte";
  import LinkDevice from "./lib/components/LinkDevice.svelte";
  import ChatLayout from "./lib/components/ChatLayout.svelte";

  let loading = $state(true);

  onMount(async () => {
    await provisioningStore.checkLinkStatus();
    loading = false;
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
