<script lang="ts">
  import { onMount } from "svelte";
  import { convertFileSrc, invoke } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";
  import { openUrl, openPath } from "@tauri-apps/plugin-opener";
  import { messagingStore } from "../stores/messaging.svelte";
  import type { ChatMessage } from "../types";
  import { settingsStore } from "../stores/settings.svelte";
  import Settings from "./Settings.svelte";
  import MediaBrowser from "./MediaBrowser.svelte";
  import { callingStore } from "../stores/calling.svelte";

  let inputText = $state("");
  let messagesContainer = $state<HTMLDivElement | undefined>(undefined);
  let showNewMessage = $state(false);
  let showSettings = $state(false);
  let showMedia = $state(false);
  let newRecipient = $state("");
  let newMessageText = $state("");
  let contactSearch = $state("");
  let showUuidInput = $state(false);
  let pendingFiles = $state<string[]>([]);
  let lightboxSrc = $state<string | null>(null);
  let replyingTo = $state<ChatMessage | null>(null);

  onMount(async () => {
    await settingsStore.load();
    await messagingStore.loadSelfId();
    await messagingStore.loadConversations();
    // No 5s poll: the backend emits `conversations-updated` whenever
    // contacts sync or a new message arrives. The messaging store re-fetches
    // on that event (see messaging.svelte.ts:initListeners).
  });

  async function selectConversation(id: string) {
    messagingStore.activeConversationId = id;
    showNewMessage = false;
    // Fire READ receipts for every inbound (not-outgoing) message in the
    // thread so the sender's client shows the blue double-check.
    await messagingStore.loadMessages(id);
    const msgs = messagingStore.getMessages(id) ?? [];
    const inboundTimestamps = msgs
      .filter((m) => !m.is_outgoing)
      .map((m) => String(m.timestamp));
    if (inboundTimestamps.length > 0) {
      try {
        await invoke("mark_conversation_read", {
          conversationId: id,
          messageTimestamps: inboundTimestamps,
        });
      } catch (e) {
        console.warn("mark_conversation_read failed:", e);
      }
    }
  }

  async function handleSend() {
    const text = inputText.trim();
    if ((!text && pendingFiles.length === 0) || !messagingStore.activeConversationId) return;
    const body = text;
    const files = [...pendingFiles];
    const convId = messagingStore.activeConversationId;
    const quote = replyingTo
      ? {
          id: replyingTo.timestamp,
          author_uuid: replyingTo.sender_id,
          text: quoteSnippet(replyingTo),
        }
      : undefined;
    inputText = "";
    pendingFiles = [];
    replyingTo = null;

    // Don't block the UI — fire and forget.
    // Errors surface as toasts via the messaging store.
    if (files.length > 0 || quote) {
      messagingStore.sendMessageWithAttachments(convId, body, files, quote).catch(() => {});
    } else {
      messagingStore.sendMessage(convId, body);
    }

    requestAnimationFrame(() => {
      if (messagesContainer) {
        messagesContainer.scrollTop = messagesContainer.scrollHeight;
      }
    });
  }

  async function handleAttachFile() {
    try {
      const result = await open({
        multiple: true,
        filters: [
          { name: "Images", extensions: ["png", "jpg", "jpeg", "gif", "webp"] },
          { name: "All Files", extensions: ["*"] },
        ],
      });
      if (result) {
        const paths = Array.isArray(result) ? result : [result];
        // Tauri 2's open() returns string|string[]|null — already unwrapped.
        pendingFiles = [...pendingFiles, ...paths];
      }
    } catch (e) {
      console.error("File picker error:", e);
    }
  }

  function removePendingFile(index: number) {
    pendingFiles = pendingFiles.filter((_, i) => i !== index);
  }

  async function handleNewMessageSend() {
    const recipient = newRecipient.trim();
    const text = newMessageText.trim();
    if (!recipient || !text) return;

    try {
      await messagingStore.sendToRecipient(recipient, text);
      // Switch to this conversation
      messagingStore.activeConversationId = recipient;
      showNewMessage = false;
      newRecipient = "";
      newMessageText = "";
      // Refresh conversations
      await messagingStore.loadConversations();
      await messagingStore.loadMessages(recipient);
    } catch {
      // Toast already pushed by sendToRecipient — keep the form open so the
      // user can correct the recipient and try again.
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  }

  function handleNewMsgKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleNewMessageSend();
    }
  }

  function formatSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }

  function fileExt(name: string): string {
    const idx = name.lastIndexOf(".");
    if (idx < 0 || idx === name.length - 1) return "FILE";
    return name.slice(idx + 1).toUpperCase().slice(0, 5);
  }

  // Highest receipt level we have for an outgoing message — drives the bubble
  // indicator (✓ sent, ✓✓ delivered, ✓✓ blue read). Returns null for incoming
  // messages or when no receipt has arrived yet (i.e. "sent" only).
  function receiptStatus(msgTimestamp: number, convId: string | null): "sent" | "delivered" | "read" {
    if (!convId) return "sent";
    const perChat = messagingStore.receipts.get(convId);
    if (!perChat) return "sent";
    const r = perChat.get(String(msgTimestamp));
    if (!r) return "sent";
    if (r.type === "viewed" || r.type === "read") return "read";
    return "delivered";
  }

  function formatTime(timestamp: number): string {
    if (!timestamp) return "";
    const date = new Date(timestamp);
    const now = new Date();
    const isToday = date.toDateString() === now.toDateString();
    if (isToday) {
      return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    }
    return date.toLocaleDateString([], { month: "short", day: "numeric" });
  }

  // Contacts (1:1 conversations) for the new-message picker. Filter by search,
  // sort alphabetically by name. We pull from the same conversations array so we
  // automatically pick up newly-synced contacts without an extra backend call.
  let pickerContacts = $derived(
    messagingStore.conversations
      .filter((c) => !c.is_group)
      .filter((c) => {
        const q = contactSearch.trim().toLowerCase();
        if (!q) return true;
        return c.name.toLowerCase().includes(q) || c.id.toLowerCase().includes(q);
      })
      .slice()
      .sort((a, b) => a.name.localeCompare(b.name))
  );

  let pickedContact = $derived(
    messagingStore.conversations.find((c) => c.id === newRecipient && !c.is_group) ?? null
  );

  function pickContact(id: string) {
    newRecipient = id;
    contactSearch = "";
    showUuidInput = false;
  }

  function clearPickedContact() {
    newRecipient = "";
    showUuidInput = false;
  }

  let activeMessages = $derived(
    (messagingStore.activeConversationId
      ? messagingStore.getMessages(messagingStore.activeConversationId)
      : []
    ).filter((m) => !messagingStore.deletions.has(String(m.timestamp)))
  );

  // Auto-scroll when messages change. messagesContainer is now $state-tracked,
  // so this effect fires both on container mount and on message-list growth.
  $effect(() => {
    const container = messagesContainer;
    const count = activeMessages.length;
    if (count > 0 && container) {
      requestAnimationFrame(() => {
        container.scrollTop = container.scrollHeight;
      });
    }
  });

  let activeConversation = $derived(
    messagingStore.conversations.find(
      (c) => c.id === messagingStore.activeConversationId
    )
  );

  function startReply(msg: ChatMessage) {
    replyingTo = msg;
  }
  function cancelReply() {
    replyingTo = null;
  }
  function quoteSnippet(msg: ChatMessage): string {
    if (msg.body) return msg.body;
    if (msg.attachments?.length) return "📎 " + (msg.attachments[0].file_name || "pièce jointe");
    return "";
  }

  const QUICK_EMOJI = ["👍", "❤️", "😂", "😮", "😢", "🙏"];
  let reactingTo = $state<number | null>(null);

  function reactionsFor(msg: ChatMessage): { emoji: string; count: number; mine: boolean }[] {
    const cid = messagingStore.activeConversationId;
    if (!cid) return [];
    const perMsg = messagingStore.reactions.get(cid)?.get(String(msg.timestamp));
    if (!perMsg) return [];
    const me = messagingStore.selfId ?? "";
    const counts = new Map<string, { count: number; mine: boolean }>();
    for (const [sender, emoji] of perMsg) {
      if (!emoji) continue;
      const c = counts.get(emoji) ?? { count: 0, mine: false };
      c.count += 1;
      if (sender === me) c.mine = true;
      counts.set(emoji, c);
    }
    return [...counts.entries()].map(([emoji, c]) => ({ emoji, ...c }));
  }

  function myReaction(msg: ChatMessage): string | null {
    const cid = messagingStore.activeConversationId;
    if (!cid) return null;
    const me = messagingStore.selfId ?? "";
    return messagingStore.reactions.get(cid)?.get(String(msg.timestamp))?.get(me) ?? null;
  }

  function toggleReaction(msg: ChatMessage, emoji: string) {
    const cid = messagingStore.activeConversationId;
    if (!cid) return;
    reactingTo = null;
    const mine = myReaction(msg);
    // Tapping your current emoji removes it; otherwise (re)set to the new one.
    const remove = mine === emoji;
    messagingStore.sendReaction(cid, msg.sender_id, msg.timestamp, emoji, remove);
  }

  async function copyMessage(msg: ChatMessage) {
    if (!msg.body) return;
    try {
      await navigator.clipboard.writeText(msg.body);
    } catch (e) {
      console.error("copy failed:", e);
    }
  }
  function deleteMessage(msg: ChatMessage) {
    const cid = messagingStore.activeConversationId;
    if (!cid) return;
    if (!confirm("Supprimer ce message pour tout le monde ?")) return;
    messagingStore.deleteForEveryone(cid, msg.timestamp);
  }

  function openLightbox(src: string) {
    lightboxSrc = src;
  }

  function closeLightbox() {
    lightboxSrc = null;
  }

  async function openAttachment(att: { local_path: string | null }) {
    if (!att.local_path) return;
    try {
      // Open in the OS default handler via the opener plugin.
      await openPath(att.local_path);
    } catch (e) {
      console.error("Open attachment failed:", e);
      // Fallback: let WebKit preview it in a new view.
      try {
        window.open(convertFileSrc(att.local_path), "_blank");
      } catch {
        /* ignore */
      }
    }
  }

  // Split a message body into plain-text and URL segments so URLs render as
  // clickable links. Matches http(s):// and bare www. hosts.
  const URL_RE = /(\bhttps?:\/\/[^\s<]+|\bwww\.[^\s<]+)/gi;
  function linkify(body: string): { text: string; href: string | null }[] {
    const out: { text: string; href: string | null }[] = [];
    let last = 0;
    for (const m of body.matchAll(URL_RE)) {
      const idx = m.index ?? 0;
      if (idx > last) out.push({ text: body.slice(last, idx), href: null });
      // Don't swallow trailing sentence punctuation into the link.
      let url = m[0];
      let trailing = "";
      while (/[).,!?;:]$/.test(url)) {
        trailing = url.slice(-1) + trailing;
        url = url.slice(0, -1);
      }
      const href = url.startsWith("www.") ? `https://${url}` : url;
      out.push({ text: url, href });
      if (trailing) out.push({ text: trailing, href: null });
      last = idx + m[0].length;
    }
    if (last < body.length) out.push({ text: body.slice(last), href: null });
    return out;
  }

  async function openExternal(href: string) {
    try {
      await openUrl(href);
    } catch (e) {
      console.error("openUrl failed:", e);
    }
  }

  // Paste an image straight into the composer: grab the bitmap off the
  // clipboard, hand the bytes to the backend for a temp file, and queue it
  // like any other attachment.
  async function handlePaste(e: ClipboardEvent) {
    const items = e.clipboardData?.items;
    if (!items) return;
    for (const item of items) {
      if (item.kind === "file" && item.type.startsWith("image/")) {
        e.preventDefault();
        const file = item.getAsFile();
        if (!file) continue;
        try {
          const buf = new Uint8Array(await file.arrayBuffer());
          const ext = item.type.split("/")[1] || "png";
          const path = await invoke<string>("save_pasted_image", {
            bytes: Array.from(buf),
            extension: ext,
          });
          pendingFiles = [...pendingFiles, path];
        } catch (err) {
          console.error("paste image failed:", err);
        }
        return;
      }
    }
  }
</script>

{#snippet avatarEl(name: string, path: string | null, extra: string)}
  {#if path}
    <img class="avatar {extra}" src={convertFileSrc(path)} alt={name} />
  {:else}
    <div class="avatar {extra}">{name[0]?.toUpperCase() ?? "?"}</div>
  {/if}
{/snippet}

<div class="layout">
  <aside class="sidebar">
    <div class="sidebar-header">
      <h1>SignalUI</h1>
      <div class="header-actions">
        <button
          class="icon-btn"
          onclick={() => (showSettings = true)}
          title="Paramètres"
          aria-label="Paramètres"
        >
          ⚙
        </button>
        <button class="new-msg-btn" onclick={() => (showNewMessage = true)} title="Nouveau message">
          +
        </button>
      </div>
    </div>
    <div class="conversations">
      {#if messagingStore.conversations.length === 0}
        <div class="empty-conversations">
          <div class="sync-indicator">
            <div class="spinner-small"></div>
            <p>Synchronisation des contacts…</p>
            <p class="sync-hint">Envoyez-vous un message pour commencer</p>
          </div>
        </div>
      {:else}
        {#each messagingStore.conversations as convo}
          <button
            class="conversation"
            class:active={messagingStore.activeConversationId === convo.id}
            onclick={() => selectConversation(convo.id)}
          >
            {@render avatarEl(convo.name, convo.avatar_path, "")}
            <div class="convo-info">
              <div class="convo-top">
                <span class="convo-name">{convo.name}</span>
                <span class="convo-time">{formatTime(convo.last_timestamp)}</span>
              </div>
              <div class="convo-last">{convo.last_message ?? ""}</div>
            </div>
          </button>
        {/each}
      {/if}
    </div>
  </aside>

  <main class="chat-area">
    {#if showNewMessage}
      <div class="chat-header">
        <h2>Nouveau message</h2>
      </div>
      <div class="new-message-form">
        <div class="form-field">
          <label for="contact-search">Destinataire</label>
          {#if pickedContact}
            <div class="picked-contact">
              {@render avatarEl(pickedContact.name, pickedContact.avatar_path, "small")}
              <div class="picked-info">
                <div class="picked-name">{pickedContact.name}</div>
                <div class="picked-uuid">{pickedContact.id}</div>
              </div>
              <button class="ghost-btn" onclick={clearPickedContact} title="Changer">×</button>
            </div>
          {:else if showUuidInput}
            <input
              id="recipient"
              type="text"
              placeholder="UUID du contact (depuis Signal)"
              bind:value={newRecipient}
            />
            <button class="link-btn" onclick={() => (showUuidInput = false)}>
              ← Choisir dans la liste
            </button>
          {:else}
            <input
              id="contact-search"
              type="text"
              placeholder="Rechercher un contact…"
              bind:value={contactSearch}
              autocomplete="off"
            />
            <div class="contact-picker">
              {#if pickerContacts.length === 0}
                <div class="contact-empty">Aucun contact — vérifiez que la sync est terminée.</div>
              {:else}
                {#each pickerContacts as c}
                  <button class="contact-item" onclick={() => pickContact(c.id)}>
                    <div class="avatar small">{c.name[0]?.toUpperCase() ?? "?"}</div>
                    <div class="contact-meta">
                      <div class="contact-name">{c.name}</div>
                      <div class="contact-uuid">{c.id}</div>
                    </div>
                  </button>
                {/each}
              {/if}
            </div>
            <div class="picker-actions">
              {#if messagingStore.selfId}
                <button class="self-btn" onclick={() => pickContact(messagingStore.selfId ?? "")}>
                  Note à moi-même
                </button>
              {/if}
              <button class="link-btn" onclick={() => (showUuidInput = true)}>
                Coller un UUID…
              </button>
            </div>
          {/if}
        </div>
        <div class="form-field">
          <label for="new-msg">Message</label>
          <input
            id="new-msg"
            type="text"
            placeholder="Tapez votre message…"
            bind:value={newMessageText}
            onkeydown={handleNewMsgKeydown}
          />
        </div>
        <div class="form-actions">
          <button class="secondary-btn" onclick={() => (showNewMessage = false)}>Annuler</button>
          <button class="primary-btn" onclick={handleNewMessageSend}>Envoyer</button>
        </div>
      </div>

    {:else if activeConversation}
      <div class="chat-header">
        {@render avatarEl(activeConversation.name, activeConversation.avatar_path, "small")}
        <h2>{activeConversation.name}</h2>
        {#if !activeConversation.is_group}
          <button
            class="icon-btn"
            onclick={() =>
              callingStore.startCall(
                activeConversation.id,
                activeConversation.name,
              )}
            disabled={callingStore.active}
            title="Appel vocal"
            aria-label="Appel vocal"
          >
            📞
          </button>
        {/if}
        <button
          class="icon-btn"
          onclick={() => (showMedia = true)}
          title="Galerie média"
          aria-label="Galerie média"
        >
          📎
        </button>
      </div>
      <div class="messages" data-testid="messages-container" bind:this={messagesContainer}>
        {#each activeMessages as msg}
          <div class="message" class:outgoing={msg.is_outgoing}>
            <div class="msg-actions">
              <button
                class="reply-action"
                title="Réagir"
                aria-label="Réagir"
                onclick={() => (reactingTo = reactingTo === msg.timestamp ? null : msg.timestamp)}
              >☺</button>
              <button
                class="reply-action"
                title="Répondre"
                aria-label="Répondre"
                onclick={() => startReply(msg)}
              >↩</button>
              {#if msg.body}
                <button
                  class="reply-action"
                  title="Copier"
                  aria-label="Copier"
                  onclick={() => copyMessage(msg)}
                >⧉</button>
              {/if}
              {#if msg.is_outgoing}
                <button
                  class="reply-action"
                  title="Supprimer pour tout le monde"
                  aria-label="Supprimer"
                  onclick={() => deleteMessage(msg)}
                >🗑</button>
              {/if}
            </div>
            {#if reactingTo === msg.timestamp}
              <div class="emoji-picker">
                {#each QUICK_EMOJI as e}
                  <button class="emoji-opt" onclick={() => toggleReaction(msg, e)}>{e}</button>
                {/each}
              </div>
            {/if}
            <div class="bubble">
              {#if msg.quote}
                <div class="quote-bar">
                  <span class="quote-author">{msg.quote.author_name}</span>
                  <span class="quote-text">{msg.quote.text}</span>
                </div>
              {/if}
              {#if msg.attachments && msg.attachments.length > 0}
                <div class="attachments">
                  {#each msg.attachments as att}
                    {#if att.mime_type.startsWith("image/") && att.local_path}
                      <button
                        type="button"
                        class="attachment-image-btn"
                        onclick={() => openLightbox(convertFileSrc(att.local_path!))}
                        aria-label={`Agrandir ${att.file_name}`}
                      >
                        <img
                          class="attachment-image"
                          src={convertFileSrc(att.local_path)}
                          alt={att.file_name}
                          loading="lazy"
                        />
                      </button>
                    {:else if att.mime_type.startsWith("image/")}
                      <div class="attachment-placeholder" data-testid="attachment-pending">
                        🖼️ {att.file_name} ({formatSize(att.size)})
                      </div>
                    {:else}
                      <div class="attachment-file" data-testid="attachment-file">
                        <span class="att-ext" aria-hidden="true">{fileExt(att.file_name)}</span>
                        <div class="att-meta">
                          <span class="att-name">{att.file_name}</span>
                          <span class="att-size">{formatSize(att.size)}</span>
                        </div>
                        {#if att.local_path}
                          <button
                            class="att-open"
                            onclick={() => openAttachment(att)}
                            aria-label={`Ouvrir ${att.file_name}`}
                          >
                            Ouvrir
                          </button>
                        {/if}
                      </div>
                    {/if}
                  {/each}
                </div>
              {/if}
              {#if msg.body}
                <p>
                  {#each linkify(msg.body) as seg}
                    {#if seg.href}
                      <a
                        href={seg.href}
                        class="msg-link"
                        onclick={(e) => {
                          e.preventDefault();
                          openExternal(seg.href!);
                        }}>{seg.text}</a>
                    {:else}{seg.text}{/if}
                  {/each}
                </p>
              {/if}
              {#if msg.previews && msg.previews.length > 0}
                {#each msg.previews as prev}
                  <button
                    class="link-preview"
                    onclick={() => openExternal(prev.url)}
                    title={prev.url}
                  >
                    {#if prev.title}<span class="lp-title">{prev.title}</span>{/if}
                    {#if prev.description}<span class="lp-desc">{prev.description}</span>{/if}
                    <span class="lp-url">{prev.url}</span>
                  </button>
                {/each}
              {/if}
              <span class="msg-time">{formatTime(msg.timestamp)}</span>
              {#if msg.is_outgoing}
                {@const r = receiptStatus(msg.timestamp, messagingStore.activeConversationId)}
                <span class="receipt receipt-{r}" title={r} aria-label={r}>
                  {#if r === "sent"}✓{:else}✓✓{/if}
                </span>
              {/if}
              {#if reactionsFor(msg).length > 0}
                <div class="reaction-chips">
                  {#each reactionsFor(msg) as chip}
                    <button
                      class="reaction-chip"
                      class:mine={chip.mine}
                      onclick={() => toggleReaction(msg, chip.emoji)}
                    >{chip.emoji}{#if chip.count > 1}<span class="rc-count">{chip.count}</span>{/if}</button>
                  {/each}
                </div>
              {/if}
            </div>
          </div>
        {/each}
      </div>
      {#if pendingFiles.length > 0}
        <div class="pending-files">
          {#each pendingFiles as file, i}
            <div class="pending-file">
              <span>{file.split("/").pop()}</span>
              <button class="remove-file" onclick={() => removePendingFile(i)}>&times;</button>
            </div>
          {/each}
        </div>
      {/if}
      {#if replyingTo}
        <div class="reply-preview">
          <div class="reply-preview-body">
            <span class="quote-author">{replyingTo.is_outgoing ? "Vous" : replyingTo.sender_name}</span>
            <span class="quote-text">{quoteSnippet(replyingTo)}</span>
          </div>
          <button class="remove-file" onclick={cancelReply} aria-label="Annuler la réponse">&times;</button>
        </div>
      {/if}
      <div class="composer">
        <button class="attach-btn" onclick={handleAttachFile} title="Joindre un fichier">
          📎
        </button>
        <input
          type="text"
          placeholder="Message…"
          bind:value={inputText}
          onkeydown={handleKeydown}
          onpaste={handlePaste}
        />
        <button class="send-btn" onclick={handleSend}>Envoyer</button>
      </div>

    {:else}
      <div class="empty-state">
        <p>Sélectionnez une conversation ou démarrez-en une nouvelle</p>
      </div>
    {/if}
  </main>
</div>

<Settings bind:open={showSettings} />

<MediaBrowser
  bind:open={showMedia}
  conversationId={messagingStore.activeConversationId}
  conversationName={activeConversation?.name ?? ""}
/>

{#if lightboxSrc}
  <div
    class="lightbox"
    role="dialog"
    aria-modal="true"
    aria-label="Pièce jointe agrandie"
    onclick={closeLightbox}
    onkeydown={(e) => { if (e.key === "Escape") closeLightbox(); }}
    tabindex="-1"
  >
    <img src={lightboxSrc} alt="" class="lightbox-img" />
  </div>
{/if}

<style>
  .link-preview {
    display: flex;
    flex-direction: column;
    gap: 2px;
    text-align: left;
    border: 1px solid var(--border, #27272a);
    border-left: 3px solid var(--accent, #3b82f6);
    border-radius: 6px;
    padding: 6px 10px;
    margin-top: 4px;
    background: rgba(127, 127, 127, 0.08);
    cursor: pointer;
    max-width: 320px;
  }
  .lp-title { font-weight: 600; font-size: 0.85rem; }
  .lp-desc {
    font-size: 0.8rem;
    color: var(--text-secondary, #a1a1aa);
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .lp-url { font-size: 0.72rem; color: var(--accent, #3b82f6); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .quote-bar {
    display: flex;
    flex-direction: column;
    gap: 1px;
    border-left: 3px solid var(--accent, #3b82f6);
    padding: 3px 8px;
    margin-bottom: 4px;
    background: rgba(127, 127, 127, 0.12);
    border-radius: 4px;
    max-width: 100%;
  }
  .quote-author {
    font-size: 0.78rem;
    font-weight: 600;
    color: var(--accent, #3b82f6);
  }
  .quote-text {
    font-size: 0.82rem;
    color: var(--text-secondary, #a1a1aa);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 280px;
  }
  .message {
    position: relative;
  }
  .msg-actions {
    position: absolute;
    top: 50%;
    transform: translateY(-50%);
    display: flex;
    gap: 2px;
    opacity: 0;
    transition: opacity 0.12s;
  }
  .message:not(.outgoing) .msg-actions {
    right: -92px;
  }
  .message.outgoing .msg-actions {
    left: -124px;
  }
  .message:hover .msg-actions {
    opacity: 1;
  }
  .reply-action {
    border: none;
    background: var(--bg-secondary, #16213e);
    color: var(--text-secondary, #a1a1aa);
    border-radius: 50%;
    width: 26px;
    height: 26px;
    cursor: pointer;
    font-size: 0.9rem;
  }
  .emoji-picker {
    position: absolute;
    top: -6px;
    z-index: 10;
    display: flex;
    gap: 2px;
    padding: 4px 6px;
    background: var(--bg-secondary, #16213e);
    border: 1px solid var(--border, #27272a);
    border-radius: 18px;
    box-shadow: 0 6px 20px rgba(0, 0, 0, 0.4);
  }
  .message:not(.outgoing) .emoji-picker { left: 0; }
  .message.outgoing .emoji-picker { right: 0; }
  .emoji-opt {
    border: none;
    background: transparent;
    font-size: 1.15rem;
    cursor: pointer;
    border-radius: 50%;
    padding: 2px 4px;
  }
  .emoji-opt:hover { background: rgba(127, 127, 127, 0.18); }
  .reaction-chips {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-top: 4px;
  }
  .reaction-chip {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    border: 1px solid var(--border, #27272a);
    background: rgba(127, 127, 127, 0.12);
    border-radius: 12px;
    padding: 1px 7px;
    font-size: 0.82rem;
    cursor: pointer;
    line-height: 1.4;
  }
  .reaction-chip.mine {
    border-color: var(--accent, #3b82f6);
    background: rgba(59, 130, 246, 0.18);
  }
  .rc-count { font-size: 0.72rem; color: var(--text-secondary, #a1a1aa); }
  .reply-preview {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 12px;
    background: var(--bg-secondary, #16213e);
    border-top: 1px solid var(--border, #27272a);
  }
  .reply-preview-body {
    display: flex;
    flex-direction: column;
    gap: 1px;
    flex: 1;
    min-width: 0;
    border-left: 3px solid var(--accent, #3b82f6);
    padding-left: 8px;
  }
  img.avatar {
    object-fit: cover;
    background: transparent;
  }
  .chat-header .avatar.small {
    margin-right: 10px;
  }
  .msg-link {
    color: var(--accent, #3b82f6);
    text-decoration: underline;
    word-break: break-all;
    cursor: pointer;
  }
  .message.outgoing .msg-link {
    color: #cfe0ff;
  }
  .receipt {
    font-size: 0.78rem;
    margin-left: 4px;
    color: var(--text-secondary, #a1a1aa);
    line-height: 1;
    vertical-align: baseline;
  }
  .receipt-read {
    color: var(--accent, #3b82f6);
  }

  .header-actions {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .icon-btn {
    background: transparent;
    color: var(--text-secondary, #a1a1aa);
    border: none;
    font-size: 1.1rem;
    line-height: 1;
    cursor: pointer;
    padding: 4px 6px;
    border-radius: 6px;
  }
  .icon-btn:hover {
    background: rgba(255, 255, 255, 0.06);
    color: var(--text-primary, #e4e4e7);
  }

  .chat-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  .chat-header h2 {
    margin: 0;
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .new-msg-btn {
    background: var(--accent, #3b82f6);
    color: white;
    border: none;
    border-radius: 50%;
    width: 28px;
    height: 28px;
    font-size: 1.2rem;
    line-height: 1;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }
  .new-msg-btn:hover {
    background: var(--accent-hover, #2563eb);
  }

  .new-message-form {
    display: flex;
    flex-direction: column;
    gap: 16px;
    padding: 24px;
    max-width: 500px;
  }

  .form-field {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .form-field label {
    font-size: 0.8rem;
    color: var(--text-secondary, #a1a1aa);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .form-field input {
    padding: 10px 12px;
    background: var(--bg-secondary, #16213e);
    border: 1px solid var(--border, #27272a);
    border-radius: 8px;
    color: var(--text-primary, #e4e4e7);
    font-size: 0.95rem;
  }

  .form-field input:focus {
    outline: none;
    border-color: var(--accent, #3b82f6);
  }

  .self-btn {
    align-self: flex-start;
    padding: 4px 10px;
    background: transparent;
    color: var(--accent, #3b82f6);
    border: 1px solid var(--accent, #3b82f6);
    border-radius: 4px;
    font-size: 0.75rem;
    cursor: pointer;
  }

  .self-btn:hover {
    background: var(--accent, #3b82f6);
    color: white;
  }

  .link-btn {
    align-self: flex-start;
    background: transparent;
    color: var(--accent, #3b82f6);
    border: none;
    padding: 4px 0;
    font-size: 0.8rem;
    cursor: pointer;
    text-decoration: underline;
  }

  .picker-actions {
    display: flex;
    gap: 12px;
    align-items: center;
    margin-top: 8px;
  }

  .contact-picker {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border, #27272a);
    border-radius: 8px;
    background: var(--bg-secondary, #16213e);
    max-height: 280px;
    overflow-y: auto;
  }

  .contact-empty {
    padding: 16px;
    color: var(--text-secondary, #a1a1aa);
    font-size: 0.85rem;
    text-align: center;
  }

  .contact-item {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 12px;
    background: transparent;
    border: none;
    border-bottom: 1px solid rgba(255,255,255,0.04);
    color: inherit;
    text-align: left;
    cursor: pointer;
    transition: background 0.1s;
  }

  .contact-item:last-child {
    border-bottom: none;
  }

  .contact-item:hover {
    background: rgba(255,255,255,0.05);
  }

  .contact-meta {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 0;
  }

  .contact-name {
    color: var(--text-primary, #e4e4e7);
    font-size: 0.9rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .contact-uuid {
    color: var(--text-secondary, #a1a1aa);
    font-size: 0.7rem;
    font-family: ui-monospace, monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .picked-contact {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
    background: var(--bg-secondary, #16213e);
    border: 1px solid var(--accent, #3b82f6);
    border-radius: 8px;
  }

  .picked-info {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 0;
  }

  .picked-name {
    color: var(--text-primary, #e4e4e7);
    font-size: 0.95rem;
    font-weight: 500;
  }

  .picked-uuid {
    color: var(--text-secondary, #a1a1aa);
    font-size: 0.7rem;
    font-family: ui-monospace, monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .ghost-btn {
    background: transparent;
    color: var(--text-secondary, #a1a1aa);
    border: none;
    font-size: 1.4rem;
    line-height: 1;
    cursor: pointer;
    padding: 4px 8px;
  }

  .ghost-btn:hover {
    color: var(--text-primary, #e4e4e7);
  }

  .avatar.small {
    width: 32px;
    height: 32px;
    font-size: 0.85rem;
    flex-shrink: 0;
  }

  .form-actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }

  .primary-btn {
    padding: 8px 20px;
    background: var(--accent, #3b82f6);
    color: white;
    border: none;
    border-radius: 8px;
    font-size: 0.9rem;
    font-weight: 600;
    cursor: pointer;
  }

  .primary-btn:hover {
    background: var(--accent-hover, #2563eb);
  }

  .secondary-btn {
    padding: 8px 20px;
    background: transparent;
    color: var(--text-secondary, #a1a1aa);
    border: 1px solid var(--border, #27272a);
    border-radius: 8px;
    font-size: 0.9rem;
    cursor: pointer;
  }

  .attachments {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 4px;
  }

  .attachment-image-btn {
    background: none;
    border: none;
    padding: 0;
    cursor: zoom-in;
    line-height: 0;
  }

  .attachment-image {
    max-width: 280px;
    max-height: 280px;
    border-radius: 8px;
    object-fit: cover;
    display: block;
  }

  .attachment-placeholder {
    background: rgba(255,255,255,0.05);
    border-radius: 8px;
    padding: 12px;
    font-size: 0.8rem;
    color: var(--text-secondary, #a1a1aa);
  }

  .attachment-file {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    background: rgba(255,255,255,0.06);
    border-radius: 8px;
    font-size: 0.85rem;
    min-width: 220px;
  }

  .att-ext {
    background: var(--accent, #3b82f6);
    color: white;
    border-radius: 4px;
    padding: 4px 6px;
    font-size: 0.7rem;
    font-weight: 700;
    letter-spacing: 0.04em;
    flex-shrink: 0;
  }

  .att-meta {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 0;
  }

  .att-name {
    color: var(--text-primary, #e4e4e7);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .att-size {
    color: var(--text-secondary, #a1a1aa);
    font-size: 0.75rem;
  }

  .att-open {
    background: transparent;
    color: var(--accent, #3b82f6);
    border: 1px solid var(--accent, #3b82f6);
    border-radius: 6px;
    padding: 4px 10px;
    font-size: 0.75rem;
    cursor: pointer;
  }

  .att-open:hover {
    background: var(--accent, #3b82f6);
    color: white;
  }

  .pending-files {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 8px 16px;
    border-top: 1px solid var(--border, #27272a);
  }

  .pending-file {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 8px;
    background: var(--bg-secondary, #16213e);
    border-radius: 4px;
    font-size: 0.8rem;
    color: var(--text-primary, #e4e4e7);
  }

  .remove-file {
    background: none;
    border: none;
    color: var(--text-secondary, #a1a1aa);
    cursor: pointer;
    font-size: 1rem;
    padding: 0 2px;
  }

  .attach-btn {
    background: none;
    border: none;
    font-size: 1.2rem;
    cursor: pointer;
    padding: 4px 8px;
    opacity: 0.7;
    transition: opacity 0.15s;
  }

  .attach-btn:hover {
    opacity: 1;
  }

  .lightbox {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.85);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 9000;
    cursor: zoom-out;
  }

  .lightbox-img {
    max-width: 92vw;
    max-height: 92vh;
    border-radius: 8px;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5);
  }
</style>
