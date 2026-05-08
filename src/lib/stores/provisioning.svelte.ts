import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ProvisioningState } from "../types";

const PROVISIONING_EVENT = "provisioning-state-changed";

export function createProvisioningStore() {
  let state = $state<ProvisioningState>({ type: "Idle" });
  let isLinked = $state(false);

  // Listen for backend state changes
  listen<ProvisioningState>(PROVISIONING_EVENT, (event) => {
    state = event.payload;
    if (state.type === "Registered") {
      isLinked = true;
    }
  });

  return {
    get state() {
      return state;
    },
    get isLinked() {
      return isLinked;
    },

    async checkLinkStatus() {
      isLinked = await invoke<boolean>("get_link_status");
      if (!isLinked) {
        state = { type: "Idle" };
      }
    },

    async startProvisioning(deviceName: string) {
      await invoke("start_provisioning", { deviceName });
    },

    async cancelProvisioning() {
      await invoke("cancel_provisioning");
    },
  };
}

export const provisioningStore = createProvisioningStore();
