import { test, expect } from "@playwright/test";

test.describe("SignalUI — Provisioning Screen", () => {
  test("shows the link device screen on first launch", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator("text=SignalUI")).toBeVisible();
    await expect(page.locator("text=Link Device")).toBeVisible();
  });

  test("has a link device button", async ({ page }) => {
    await page.goto("/");
    const btn = page.locator("button", { hasText: "Link Device" });
    await expect(btn).toBeVisible();
    await expect(btn).toBeEnabled();
  });

  test("shows lightweight Signal description", async ({ page }) => {
    await page.goto("/");
    await expect(
      page.locator("text=A lightweight Signal desktop client")
    ).toBeVisible();
  });
});
