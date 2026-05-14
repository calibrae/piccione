<script lang="ts">
  import { callingStore } from "../stores/calling.svelte";

  // One overlay, state-driven: ringing → accept/decline, dialing/connected
  // → in-call. Idle/ended → nothing (the store clears `ended` after a beat).
  let call = $derived(callingStore.call);

  // Live call-duration ticker, only while connected.
  let now = $state(Date.now());
  $effect(() => {
    if (call.state !== "connected") return;
    const t = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(t);
  });
  let duration = $derived.by(() => {
    const start = callingStore.connectedAt;
    if (call.state !== "connected" || start === null) return "";
    const secs = Math.max(0, Math.floor((now - start) / 1000));
    const m = Math.floor(secs / 60);
    const s = secs % 60;
    return `${m}:${s.toString().padStart(2, "0")}`;
  });

  function peerName(): string {
    if (
      call.state === "ringing" ||
      call.state === "dialing" ||
      call.state === "connected"
    ) {
      return call.peer_name || call.peer_uuid;
    }
    return "";
  }

  function initial(): string {
    const n = peerName();
    return n ? n[0].toUpperCase() : "?";
  }
</script>

{#if call.state !== "idle"}
  <div class="call-overlay" role="dialog" aria-label="Appel vocal">
    <div class="call-card">
      <div class="call-avatar">{initial()}</div>

      {#if call.state === "ringing"}
        <p class="call-peer">{peerName()}</p>
        <p class="call-status">Appel entrant…</p>
        <div class="call-actions">
          <button class="call-btn decline" onclick={() => callingStore.decline()}>
            <span class="call-icon">✕</span>
            Refuser
          </button>
          <button class="call-btn accept" onclick={() => callingStore.accept()}>
            <span class="call-icon">✆</span>
            Accepter
          </button>
        </div>
      {:else if call.state === "dialing"}
        <p class="call-peer">{peerName()}</p>
        <p class="call-status">Appel en cours…</p>
        <div class="call-actions">
          <button class="call-btn decline" onclick={() => callingStore.end()}>
            <span class="call-icon">✕</span>
            Annuler
          </button>
        </div>
      {:else if call.state === "connected"}
        <p class="call-peer">{peerName()}</p>
        <p class="call-status">
          {duration}
          {#if callingStore.remoteMuted}<span class="muted-tag">· muet</span>{/if}
        </p>
        <div class="call-actions">
          <button class="call-btn decline" onclick={() => callingStore.end()}>
            <span class="call-icon">✕</span>
            Raccrocher
          </button>
        </div>
      {:else if call.state === "ended"}
        <p class="call-peer">{peerName()}</p>
        <p class="call-status">Appel terminé</p>
      {/if}
    </div>
  </div>
{/if}

<style>
  .call-overlay {
    position: fixed;
    inset: 0;
    z-index: 2000;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(0, 0, 0, 0.78);
    backdrop-filter: blur(6px);
  }
  .call-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 14px;
    padding: 40px 56px;
    background: var(--bg-secondary, #16213e);
    border: 1px solid var(--border, #27272a);
    border-radius: 18px;
    box-shadow: 0 24px 70px rgba(0, 0, 0, 0.6);
    min-width: 320px;
  }
  .call-avatar {
    width: 96px;
    height: 96px;
    border-radius: 50%;
    background: var(--accent, #3b82f6);
    color: white;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 2.6rem;
    font-weight: 600;
  }
  .call-peer {
    font-size: 1.3rem;
    font-weight: 600;
    color: var(--text-primary, #e4e4e7);
    margin: 0;
    max-width: 320px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .call-status {
    font-size: 0.95rem;
    color: var(--text-secondary, #a1a1aa);
    margin: 0;
    font-variant-numeric: tabular-nums;
  }
  .muted-tag {
    color: #fca5a5;
  }
  .call-actions {
    display: flex;
    gap: 16px;
    margin-top: 8px;
  }
  .call-btn {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    border: none;
    border-radius: 12px;
    padding: 14px 22px;
    font-size: 0.85rem;
    font-weight: 600;
    color: white;
    cursor: pointer;
  }
  .call-icon {
    font-size: 1.4rem;
    line-height: 1;
  }
  .call-btn.accept {
    background: #16a34a;
  }
  .call-btn.accept:hover {
    background: #15803d;
  }
  .call-btn.decline {
    background: #dc2626;
  }
  .call-btn.decline:hover {
    background: #b91c1c;
  }
</style>
