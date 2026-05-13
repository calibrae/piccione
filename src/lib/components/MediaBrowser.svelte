<script lang="ts">
  import { invoke, convertFileSrc } from "@tauri-apps/api/core";
  import type { AttachmentInfo } from "../types";

  interface MediaItem {
    timestamp: number;
    sender_id: string;
    sender_name: string;
    is_outgoing: boolean;
    attachment: AttachmentInfo;
  }

  let {
    open = $bindable(false),
    conversationId,
    conversationName,
  }: {
    open: boolean;
    conversationId: string | null;
    conversationName: string;
  } = $props();

  let items = $state<MediaItem[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let lightboxSrc = $state<string | null>(null);

  // Reload whenever the modal opens for a different conversation.
  let lastLoadedFor = $state<string | null>(null);
  $effect(() => {
    if (open && conversationId && conversationId !== lastLoadedFor) {
      lastLoadedFor = conversationId;
      void load(conversationId);
    } else if (!open) {
      // Drop cached items when closing — small win on RAM for big libraries.
      items = [];
      lastLoadedFor = null;
    }
  });

  async function load(convId: string) {
    loading = true;
    error = null;
    try {
      items = await invoke<MediaItem[]>("get_conversation_media", {
        conversationId: convId,
      });
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  let images = $derived(items.filter((m) => m.attachment.mime_type.startsWith("image/")));
  let files = $derived(items.filter((m) => !m.attachment.mime_type.startsWith("image/")));

  function formatSize(b: number): string {
    if (b < 1024) return `${b} B`;
    if (b < 1024 * 1024) return `${(b / 1024).toFixed(1)} KB`;
    return `${(b / (1024 * 1024)).toFixed(1)} MB`;
  }
  function formatDate(ts: number): string {
    return new Date(ts).toLocaleDateString([], {
      year: "numeric",
      month: "short",
      day: "numeric",
    });
  }
  function fileExt(name: string): string {
    const i = name.lastIndexOf(".");
    if (i < 0 || i === name.length - 1) return "FILE";
    return name.slice(i + 1).toUpperCase().slice(0, 5);
  }

  async function openFile(path: string) {
    try {
      // Tauri's openPath isn't on the dialog plugin — easiest is shell open.
      // For attachments we want "reveal in Finder" + "open with default app".
      // Spawn /usr/bin/open via Command (if plugin available); otherwise fall
      // back to converting to a file URL the WebView can navigate to.
      const fileUrl = convertFileSrc(path);
      window.open(fileUrl, "_blank");
    } catch (e) {
      console.error("openFile failed:", e);
    }
  }
</script>

{#if open}
  <button
    type="button"
    class="overlay"
    onclick={() => (open = false)}
    aria-label="Fermer la galerie"
  ></button>

  <div class="panel" role="dialog" aria-label="Galerie media">
    <header class="panel-header">
      <div>
        <h2>Médias</h2>
        <p class="subtitle">{conversationName}</p>
      </div>
      <button class="close-btn" onclick={() => (open = false)} aria-label="Fermer">×</button>
    </header>

    <div class="body">
      {#if loading}
        <p class="empty">Chargement…</p>
      {:else if error}
        <p class="error">Erreur : {error}</p>
      {:else if items.length === 0}
        <p class="empty">Aucun média échangé dans cette conversation.</p>
      {:else}
        {#if images.length > 0}
          <section class="section">
            <h3>Images <span class="count">{images.length}</span></h3>
            <div class="grid">
              {#each images as m (m.timestamp + "-" + m.attachment.id)}
                {#if m.attachment.local_path}
                  <button
                    type="button"
                    class="thumb"
                    title={`${m.sender_name} · ${formatDate(m.timestamp)}`}
                    onclick={() => (lightboxSrc = convertFileSrc(m.attachment.local_path!))}
                  >
                    <img
                      src={convertFileSrc(m.attachment.local_path)}
                      alt={m.attachment.file_name}
                      loading="lazy"
                    />
                  </button>
                {:else}
                  <div class="thumb thumb-pending" title="Pas encore téléchargé">
                    <span>🖼️</span>
                  </div>
                {/if}
              {/each}
            </div>
          </section>
        {/if}

        {#if files.length > 0}
          <section class="section">
            <h3>Fichiers <span class="count">{files.length}</span></h3>
            <ul class="files">
              {#each files as m (m.timestamp + "-" + m.attachment.id)}
                <li class="file-row">
                  <span class="file-ext">{fileExt(m.attachment.file_name)}</span>
                  <div class="file-meta">
                    <span class="file-name">{m.attachment.file_name}</span>
                    <span class="file-sub">
                      {formatSize(m.attachment.size)} ·
                      {m.sender_name} ·
                      {formatDate(m.timestamp)}
                    </span>
                  </div>
                  {#if m.attachment.local_path}
                    <button
                      class="file-open"
                      onclick={() => openFile(m.attachment.local_path!)}
                    >
                      Ouvrir
                    </button>
                  {:else}
                    <span class="file-pending">Non téléchargé</span>
                  {/if}
                </li>
              {/each}
            </ul>
          </section>
        {/if}
      {/if}
    </div>
  </div>

  {#if lightboxSrc}
    <button
      type="button"
      class="lightbox"
      onclick={() => (lightboxSrc = null)}
      aria-label="Fermer l'image"
    >
      <img src={lightboxSrc} alt="" />
    </button>
  {/if}
{/if}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    border: none;
    padding: 0;
    cursor: pointer;
    z-index: 999;
  }
  .panel {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: min(820px, 94vw);
    max-height: 88vh;
    display: flex;
    flex-direction: column;
    background: var(--bg-primary, #0f0f1a);
    border: 1px solid var(--border, #27272a);
    border-radius: 12px;
    z-index: 1000;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5);
  }
  .panel-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    padding: 18px 22px;
    border-bottom: 1px solid var(--border, #27272a);
  }
  .panel-header h2 {
    font-size: 1.1rem;
    margin: 0;
  }
  .subtitle {
    color: var(--text-secondary, #a1a1aa);
    font-size: 0.8rem;
    margin: 4px 0 0 0;
    max-width: 460px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .close-btn {
    background: transparent;
    border: none;
    font-size: 1.6rem;
    line-height: 1;
    color: var(--text-secondary, #a1a1aa);
    cursor: pointer;
    padding: 0 6px;
  }
  .body {
    padding: 18px 22px;
    overflow-y: auto;
    flex: 1;
  }
  .empty, .error {
    color: var(--text-secondary, #a1a1aa);
    padding: 36px 0;
    text-align: center;
    font-size: 0.9rem;
  }
  .error {
    color: #f87171;
  }
  .section {
    margin-bottom: 24px;
  }
  .section h3 {
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.07em;
    color: var(--text-secondary, #a1a1aa);
    margin: 0 0 10px 0;
    display: flex;
    align-items: baseline;
    gap: 8px;
  }
  .count {
    color: var(--text-secondary, #a1a1aa);
    font-size: 0.7rem;
    font-weight: normal;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(120px, 1fr));
    gap: 6px;
  }
  .thumb {
    aspect-ratio: 1;
    border: none;
    padding: 0;
    background: var(--bg-secondary, #16213e);
    border-radius: 6px;
    overflow: hidden;
    cursor: zoom-in;
    line-height: 0;
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .thumb-pending {
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: default;
    font-size: 1.5rem;
  }
  .files {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .file-row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
    background: var(--bg-secondary, #16213e);
    border-radius: 8px;
  }
  .file-ext {
    background: var(--accent, #3b82f6);
    color: white;
    border-radius: 4px;
    padding: 4px 6px;
    font-size: 0.7rem;
    font-weight: 700;
    letter-spacing: 0.04em;
    flex-shrink: 0;
  }
  .file-meta {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .file-name {
    color: var(--text-primary, #e4e4e7);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .file-sub {
    color: var(--text-secondary, #a1a1aa);
    font-size: 0.75rem;
  }
  .file-open {
    background: transparent;
    color: var(--accent, #3b82f6);
    border: 1px solid var(--accent, #3b82f6);
    border-radius: 6px;
    padding: 4px 12px;
    font-size: 0.78rem;
    cursor: pointer;
  }
  .file-open:hover {
    background: var(--accent, #3b82f6);
    color: white;
  }
  .file-pending {
    color: var(--text-secondary, #a1a1aa);
    font-size: 0.75rem;
    font-style: italic;
  }
  .lightbox {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.92);
    border: none;
    padding: 0;
    cursor: zoom-out;
    z-index: 1100;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .lightbox img {
    max-width: 92vw;
    max-height: 92vh;
    object-fit: contain;
  }
</style>
