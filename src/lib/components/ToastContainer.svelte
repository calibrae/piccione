<script lang="ts">
  import { toastStore } from "../stores/toasts.svelte";

  async function handleRetry(id: number, retry?: () => void | Promise<void>) {
    toastStore.dismiss(id);
    if (retry) {
      try {
        await retry();
      } catch (e) {
        // Surface the secondary error too, but don't loop.
        toastStore.error(`Échec de l'envoi : ${String(e)}`);
      }
    }
  }
</script>

<div class="toast-container" role="region" aria-live="polite" aria-label="Notifications">
  {#each toastStore.list as toast (toast.id)}
    <div class="toast toast-{toast.kind}" role={toast.kind === "error" ? "alert" : "status"}>
      <span class="toast-message">{toast.message}</span>
      {#if toast.retry}
        <button
          class="toast-action"
          onclick={() => handleRetry(toast.id, toast.retry)}
          aria-label="Réessayer"
        >
          Réessayer
        </button>
      {/if}
      <button
        class="toast-dismiss"
        onclick={() => toastStore.dismiss(toast.id)}
        aria-label="Fermer"
      >
        &times;
      </button>
    </div>
  {/each}
</div>

<style>
  .toast-container {
    position: fixed;
    bottom: 16px;
    right: 16px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    z-index: 9999;
    pointer-events: none;
    max-width: min(420px, calc(100vw - 32px));
  }

  .toast {
    pointer-events: auto;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 14px;
    border-radius: 10px;
    background: var(--bg-secondary, #16213e);
    color: var(--text-primary, #e4e4e7);
    border: 1px solid var(--border, #27272a);
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.35);
    font-size: 0.9rem;
    animation: toast-in 0.18s ease-out;
  }

  .toast-error {
    border-color: #ef4444;
    background: rgba(239, 68, 68, 0.12);
  }

  .toast-success {
    border-color: #22c55e;
    background: rgba(34, 197, 94, 0.12);
  }

  .toast-message {
    flex: 1;
    line-height: 1.3;
    word-break: break-word;
  }

  .toast-action {
    background: var(--accent, #3b82f6);
    color: white;
    border: none;
    border-radius: 6px;
    padding: 4px 10px;
    font-size: 0.8rem;
    font-weight: 600;
    cursor: pointer;
  }

  .toast-action:hover {
    background: var(--accent-hover, #2563eb);
  }

  .toast-dismiss {
    background: transparent;
    color: var(--text-secondary, #a1a1aa);
    border: none;
    cursor: pointer;
    font-size: 1.1rem;
    line-height: 1;
    padding: 0 4px;
  }

  .toast-dismiss:hover {
    color: var(--text-primary, #e4e4e7);
  }

  @keyframes toast-in {
    from {
      opacity: 0;
      transform: translateY(8px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }
</style>
