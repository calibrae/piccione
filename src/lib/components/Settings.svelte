<script lang="ts">
  import { settingsStore, type Theme } from "../stores/settings.svelte";
  import { messagingStore } from "../stores/messaging.svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { open as openDialog } from "@tauri-apps/plugin-dialog";

  interface DeviceDto {
    id: number;
    name: string | null;
    created_at: number;
    last_seen: number;
    is_current: boolean;
  }

  let { open = $bindable(false) }: { open: boolean } = $props();
  let devices = $state<DeviceDto[]>([]);
  let devicesLoading = $state(false);
  let devicesError = $state<string | null>(null);

  let profileGiven = $state("");
  let profileFamily = $state("");
  let profileAbout = $state("");
  let profileSaving = $state(false);
  let profileMsg = $state<string | null>(null);

  async function saveProfile() {
    profileSaving = true;
    profileMsg = null;
    try {
      await invoke("set_profile", {
        givenName: profileGiven.trim(),
        familyName: profileFamily.trim() || null,
        about: profileAbout.trim() || null,
      });
      profileMsg = "Profil mis à jour";
    } catch (e) {
      profileMsg = "Erreur : " + String(e);
    } finally {
      profileSaving = false;
    }
  }

  let backupReady = $state(false);
  let importing = $state(false);
  let importMsg = $state<string | null>(null);

  async function refreshBackupReady() {
    try {
      backupReady = await invoke<boolean>("backup_available");
    } catch {
      backupReady = false;
    }
  }

  async function importBackup() {
    importMsg = null;
    let path: string | null = null;
    try {
      const sel = await openDialog({ multiple: false, title: "Choisir une archive de transfert" });
      path = Array.isArray(sel) ? sel[0] ?? null : sel;
    } catch (e) {
      importMsg = "Erreur sélecteur : " + String(e);
      return;
    }
    if (!path) return;
    importing = true;
    try {
      const n = await invoke<number>("import_backup", { path });
      importMsg = `Import : ${n} contact(s).`;
    } catch (e) {
      importMsg = "Échec : " + String(e);
    } finally {
      importing = false;
    }
  }

  async function loadDevices() {
    devicesLoading = true;
    devicesError = null;
    try {
      devices = await invoke<DeviceDto[]>("list_devices");
    } catch (e) {
      devicesError = String(e);
    } finally {
      devicesLoading = false;
    }
  }

  function fmtDate(ms: number): string {
    if (!ms) return "—";
    return new Date(ms).toLocaleString([], {
      day: "numeric",
      month: "short",
      hour: "2-digit",
      minute: "2-digit",
    });
  }

  // Load devices when the panel opens.
  $effect(() => {
    if (open && devices.length === 0 && !devicesLoading) {
      void loadDevices();
      void refreshBackupReady();
    }
  });
  let signingOut = $state(false);
  let confirmSignOut = $state(false);
  let saveError = $state<string | null>(null);

  // Local mirrors so we can debounce / show optimistic state.
  let readReceipts = $derived(settingsStore.current.read_receipts);
  let typingIndicators = $derived(settingsStore.current.typing_indicators);
  let theme = $derived(settingsStore.current.theme);

  async function toggle(key: "read_receipts" | "typing_indicators", v: boolean) {
    saveError = null;
    try {
      await settingsStore.update({ [key]: v });
    } catch (e) {
      saveError = String(e);
    }
  }

  async function setTheme(t: Theme) {
    saveError = null;
    try {
      await settingsStore.update({ theme: t });
    } catch (e) {
      saveError = String(e);
    }
  }

  async function doSignOut() {
    signingOut = true;
    try {
      await settingsStore.signOut();
      // Reload so the link-device screen reappears with fresh state.
      window.location.reload();
    } catch (e) {
      saveError = String(e);
      signingOut = false;
    }
  }
</script>

{#if open}
  <button
    type="button"
    class="overlay"
    onclick={() => (open = false)}
    aria-label="Fermer les paramètres"
  ></button>

  <div class="panel" role="dialog" aria-label="Paramètres">
    <header class="panel-header">
      <h2>Paramètres</h2>
      <button class="close-btn" onclick={() => (open = false)} aria-label="Fermer">×</button>
    </header>

    <section class="group">
      <h3>Confidentialité</h3>
      <label class="row">
        <div class="row-text">
          <span class="row-title">Accusés de lecture</span>
          <span class="row-desc">
            Renvoyer ✓✓ et ✓✓ bleu à l'expéditeur. Désactivez pour rester invisible — vous recevez
            les accusés des autres mais n'en envoyez plus.
          </span>
        </div>
        <input
          type="checkbox"
          checked={readReceipts}
          onchange={(e) => toggle("read_receipts", (e.target as HTMLInputElement).checked)}
        />
      </label>

      <label class="row">
        <div class="row-text">
          <span class="row-title">Indicateurs de frappe</span>
          <span class="row-desc">
            Afficher « en train d'écrire » à votre interlocuteur. (Pas encore émis par signalui ;
            réservé pour la prochaine version.)
          </span>
        </div>
        <input
          type="checkbox"
          checked={typingIndicators}
          onchange={(e) => toggle("typing_indicators", (e.target as HTMLInputElement).checked)}
        />
      </label>
    </section>

    <section class="group">
      <h3>Apparence</h3>
      <div class="theme-row">
        {#each ["light", "dark", "auto"] as t (t)}
          <button
            class="theme-btn"
            class:active={theme === t}
            onclick={() => setTheme(t as Theme)}
          >
            {t === "light" ? "Clair" : t === "dark" ? "Sombre" : "Système"}
          </button>
        {/each}
      </div>
    </section>

    <section class="group">
      <h3>Sauvegarde</h3>
      {#if backupReady}
        <p class="muted small">Importer l'historique depuis une archive de transfert exportée par votre téléphone (contacts pour l'instant ; messages à venir).</p>
        <button class="secondary-btn" onclick={importBackup} disabled={importing}>
          {importing ? "Import…" : "Importer une sauvegarde"}
        </button>
        {#if importMsg}<p class="muted small">{importMsg}</p>{/if}
      {:else}
        <p class="muted small">Sauvegardes indisponibles (cet appareil n'a pas reçu de clé de sauvegarde au moment de la liaison).</p>
      {/if}
    </section>

    <section class="group">
      <h3>Profil</h3>
      <div class="profile-fields">
        <input type="text" placeholder="Prénom" bind:value={profileGiven} maxlength="64" />
        <input type="text" placeholder="Nom (facultatif)" bind:value={profileFamily} maxlength="64" />
        <input type="text" placeholder="À propos (facultatif)" bind:value={profileAbout} maxlength="140" />
        <button class="primary-btn" onclick={saveProfile} disabled={profileSaving || !profileGiven.trim()}>
          {profileSaving ? "Enregistrement…" : "Enregistrer le profil"}
        </button>
        {#if profileMsg}<p class="muted small">{profileMsg}</p>{/if}
      </div>
    </section>

    <section class="group">
      <h3>Appareils liés</h3>
      {#if devicesLoading}
        <p class="muted">Chargement…</p>
      {:else if devicesError}
        <p class="error">Erreur : {devicesError}</p>
      {:else if devices.length === 0}
        <p class="muted">Aucun appareil.</p>
      {:else}
        <ul class="device-list">
          {#each devices as d}
            <li class="device">
              <div class="device-info">
                <span class="device-name">{d.name || `Appareil ${d.id}`}{#if d.is_current} <em>(cet appareil)</em>{/if}</span>
                <span class="device-meta">Vu : {fmtDate(d.last_seen)} · Lié : {fmtDate(d.created_at)}</span>
              </div>
            </li>
          {/each}
        </ul>
        <p class="muted small">Pour délier un appareil, utilisez votre téléphone (appareil principal).</p>
      {/if}
    </section>

    <section class="group">
      <h3>Compte</h3>
      <dl class="info">
        <dt>ACI</dt>
        <dd class="mono">{messagingStore.selfId ?? "—"}</dd>
      </dl>
      {#if !confirmSignOut}
        <button class="danger-btn" onclick={() => (confirmSignOut = true)}>Se déconnecter</button>
      {:else}
        <div class="confirm-block">
          <p class="warn">
            Cela supprime la clé de chiffrement et les données locales. Vous devrez réscanner le QR
            pour relier l'appareil.
          </p>
          <div class="confirm-actions">
            <button class="secondary-btn" onclick={() => (confirmSignOut = false)} disabled={signingOut}>
              Annuler
            </button>
            <button class="danger-btn" onclick={doSignOut} disabled={signingOut}>
              {signingOut ? "Déconnexion…" : "Confirmer"}
            </button>
          </div>
        </div>
      {/if}
    </section>

    {#if saveError}
      <p class="error">Erreur : {saveError}</p>
    {/if}
  </div>
{/if}

<style>
  .profile-fields { display: flex; flex-direction: column; gap: 8px; }
  .profile-fields input {
    padding: 8px 10px;
    border: 1px solid var(--border, #27272a);
    border-radius: 8px;
    background: var(--bg-primary, #0f0f1a);
    color: var(--text-primary, #e4e4e7);
    font-size: 0.88rem;
  }
  .device-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 8px; }
  .device { display: flex; align-items: center; gap: 10px; padding: 8px 10px; border: 1px solid var(--border, #27272a); border-radius: 8px; }
  .device-info { display: flex; flex-direction: column; gap: 2px; }
  .device-name { font-weight: 600; font-size: 0.9rem; }
  .device-name em { color: var(--accent, #3b82f6); font-style: normal; font-weight: 400; font-size: 0.8rem; }
  .device-meta { font-size: 0.76rem; color: var(--text-secondary, #a1a1aa); }
  .muted { color: var(--text-secondary, #a1a1aa); font-size: 0.85rem; }
  .muted.small { font-size: 0.74rem; margin-top: 6px; }

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
    width: min(520px, 92vw);
    max-height: 86vh;
    overflow-y: auto;
    background: var(--bg-primary, #0f0f1a);
    border: 1px solid var(--border, #27272a);
    border-radius: 12px;
    padding: 0;
    z-index: 1000;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5);
  }
  .panel-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 18px 22px;
    border-bottom: 1px solid var(--border, #27272a);
  }
  .panel-header h2 {
    font-size: 1.1rem;
    margin: 0;
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
  .close-btn:hover {
    color: var(--text-primary, #e4e4e7);
  }
  .group {
    padding: 18px 22px;
    border-bottom: 1px solid var(--border, #27272a);
  }
  .group:last-of-type {
    border-bottom: none;
  }
  .group h3 {
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.07em;
    color: var(--text-secondary, #a1a1aa);
    margin: 0 0 12px 0;
  }
  .row {
    display: flex;
    align-items: flex-start;
    gap: 16px;
    padding: 10px 0;
    cursor: pointer;
  }
  .row-text {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .row-title {
    color: var(--text-primary, #e4e4e7);
    font-size: 0.95rem;
  }
  .row-desc {
    color: var(--text-secondary, #a1a1aa);
    font-size: 0.8rem;
    line-height: 1.45;
  }
  .row input[type="checkbox"] {
    margin-top: 4px;
    width: 18px;
    height: 18px;
    accent-color: var(--accent, #3b82f6);
  }
  .theme-row {
    display: flex;
    gap: 8px;
  }
  .theme-btn {
    flex: 1;
    padding: 8px 12px;
    background: var(--bg-secondary, #16213e);
    border: 1px solid var(--border, #27272a);
    border-radius: 8px;
    color: var(--text-primary, #e4e4e7);
    font-size: 0.9rem;
    cursor: pointer;
  }
  .theme-btn:hover {
    border-color: var(--accent, #3b82f6);
  }
  .theme-btn.active {
    background: var(--accent, #3b82f6);
    border-color: var(--accent, #3b82f6);
    color: white;
  }
  .info {
    display: grid;
    grid-template-columns: auto 1fr;
    column-gap: 12px;
    row-gap: 4px;
    margin: 0 0 14px 0;
  }
  .info dt {
    color: var(--text-secondary, #a1a1aa);
    font-size: 0.8rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .info dd {
    margin: 0;
    color: var(--text-primary, #e4e4e7);
    font-size: 0.9rem;
    overflow-wrap: anywhere;
  }
  .info .mono {
    font-family: ui-monospace, monospace;
    font-size: 0.78rem;
  }
  .danger-btn {
    background: transparent;
    color: #f87171;
    border: 1px solid #f87171;
    border-radius: 8px;
    padding: 8px 18px;
    font-size: 0.9rem;
    cursor: pointer;
  }
  .danger-btn:hover {
    background: #f87171;
    color: white;
  }
  .danger-btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }
  .secondary-btn {
    background: transparent;
    color: var(--text-secondary, #a1a1aa);
    border: 1px solid var(--border, #27272a);
    border-radius: 8px;
    padding: 8px 18px;
    font-size: 0.9rem;
    cursor: pointer;
  }
  .confirm-block {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .warn {
    color: #fca5a5;
    font-size: 0.85rem;
    margin: 0;
    line-height: 1.45;
  }
  .confirm-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .error {
    color: #f87171;
    font-size: 0.85rem;
    padding: 0 22px 18px 22px;
    margin: 0;
  }
</style>
