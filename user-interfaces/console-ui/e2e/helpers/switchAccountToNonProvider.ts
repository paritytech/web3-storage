import { expect, type Page } from "@playwright/test";

export const switchAccountHandler = async (localPage: Page, who: string = "Bob") => {
    await localPage.getByTestId("nav-accounts").click();
    await expect(localPage.getByTestId("accounts-list")).toBeVisible();

    // Default active should be Alice (auto-set on local).
    await expect(localPage.getByTestId("accounts-active-badge-Alice")).toBeVisible({
        timeout: 30_000,
    });

    await localPage.getByTestId("accounts-set-active-Bob").click();
    await expect(localPage.getByTestId("accounts-active-badge-Bob")).toBeVisible({
        timeout: 30_000,
    });
    // Sidebar signer-name reflects the new account.
    await expect(localPage.getByTestId("signer-name")).toHaveText("Bob");
}