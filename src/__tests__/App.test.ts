import { render, screen } from "@testing-library/svelte";
import { describe, it, expect, vi } from "vitest";

// Mock Tauri APIs before importing components
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(false),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

import App from "../App.svelte";

describe("App", () => {
  it("renders the provisioning screen when not linked", async () => {
    render(App);
    // Wait for loading to finish
    const linkButton = await screen.findByText("Link Device", {}, { timeout: 2000 });
    expect(linkButton).toBeInTheDocument();
  });
});
