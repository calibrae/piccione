<script lang="ts">
  import { provisioningStore } from "../stores/provisioning.svelte";
  import QrCode from "./QrCode.svelte";

  const DEFAULT_DEVICE_NAME = "SignalUI Desktop";

  let started = $state(false);

  async function handleLink() {
    started = true;
    await provisioningStore.startProvisioning(DEFAULT_DEVICE_NAME);
  }

  function handleCancel() {
    provisioningStore.cancelProvisioning();
    started = false;
  }

  function handleRetry() {
    started = false;
  }
</script>

<div class="link-device">
  <div class="link-content">
    {#if provisioningStore.state.type === "Idle" && !started}
      <div class="welcome">
        <h1>SignalUI</h1>
        <p class="subtitle">A lightweight Signal desktop client</p>
        <button class="primary-btn" onclick={handleLink}>
          Link Device
        </button>
      </div>

    {:else if provisioningStore.state.type === "Connecting"}
      <div class="status">
        <div class="spinner"></div>
        <p>Connecting to Signal...</p>
      </div>

    {:else if provisioningStore.state.type === "WaitingForScan"}
      <div class="scan">
        <h2>Scan QR Code</h2>
        <p class="instruction">
          Open Signal on your phone, go to Settings &rarr; Linked Devices &rarr; Link New Device
        </p>
        <QrCode svg={provisioningStore.state.qr_code_svg} />
        <button class="secondary-btn" onclick={handleCancel}>
          Cancel
        </button>
      </div>

    {:else if provisioningStore.state.type === "Provisioning"}
      <div class="status">
        <div class="spinner"></div>
        <p>Linking device...</p>
      </div>

    {:else if provisioningStore.state.type === "Registered"}
      <div class="status success">
        <div class="checkmark">&#10003;</div>
        <p>Device linked successfully!</p>
      </div>

    {:else if provisioningStore.state.type === "Error"}
      <div class="status error">
        <p class="error-msg">{provisioningStore.state.message}</p>
        <button class="primary-btn" onclick={handleRetry}>
          Try Again
        </button>
      </div>

    {:else}
      <div class="status">
        <div class="spinner"></div>
        <p>Starting...</p>
      </div>
    {/if}
  </div>
</div>

<style>
  .link-device {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 100%;
    height: 100%;
    background: var(--bg-primary, #0f0f1a);
  }

  .link-content {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 24px;
    max-width: 400px;
    text-align: center;
  }

  .welcome h1 {
    font-size: 2rem;
    font-weight: 700;
    margin: 0 0 8px;
    color: var(--text-primary, #e4e4e7);
  }

  .subtitle {
    color: var(--text-secondary, #a1a1aa);
    margin: 0 0 32px;
    font-size: 1rem;
  }

  .scan {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 20px;
  }

  .scan h2 {
    margin: 0;
    font-size: 1.5rem;
    color: var(--text-primary, #e4e4e7);
  }

  .instruction {
    color: var(--text-secondary, #a1a1aa);
    font-size: 0.9rem;
    margin: 0;
    max-width: 320px;
    line-height: 1.5;
  }

  .status {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 16px;
  }

  .status p {
    color: var(--text-secondary, #a1a1aa);
    margin: 0;
  }

  .error-msg {
    color: #ef4444 !important;
  }

  .success .checkmark {
    font-size: 3rem;
    color: #22c55e;
  }

  .spinner {
    width: 32px;
    height: 32px;
    border: 3px solid var(--border, #27272a);
    border-top-color: var(--accent, #3b82f6);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .primary-btn {
    padding: 12px 32px;
    background: var(--accent, #3b82f6);
    color: white;
    border: none;
    border-radius: 8px;
    font-size: 1rem;
    font-weight: 600;
    cursor: pointer;
    transition: background 0.15s;
  }

  .primary-btn:hover {
    background: #2563eb;
  }

  .secondary-btn {
    padding: 8px 20px;
    background: transparent;
    color: var(--text-secondary, #a1a1aa);
    border: 1px solid var(--border, #27272a);
    border-radius: 8px;
    font-size: 0.9rem;
    cursor: pointer;
    transition: border-color 0.15s;
  }

  .secondary-btn:hover {
    border-color: var(--text-secondary, #a1a1aa);
  }
</style>
