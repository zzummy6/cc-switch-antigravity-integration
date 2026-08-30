import { test, expect, type Page } from "@playwright/test";

/**
 * Antigravity provider E2E（浏览器级：vite dev renderer + mock Tauri IPC）。
 *
 * Tauri v2 前端的 `invoke` 走 `window.__TAURI_INTERNALS__.invoke`，这里注入
 * stub：已知命令返回可控 mock，未知命令返回 null/[]，让完整 App 可以挂载。
 * 验证路径：添加供应商 → 搜索 Antigravity 预设 → 表单出现 Google 登录区块。
 */

type InvokeHandler = (cmd: string, args: Record<string, unknown>) => unknown;

function installTauriMock(page: Page, overrides: Record<string, InvokeHandler> = {}) {
  void page.addInitScript(
    ({ overridesJson }: { overridesJson: string }) => {
      // Playwright 的 addInitScript 参数已结构化克隆，overridesJson 解析后
      // 的值就是对象本身（不是字符串），直接 structuredClone 返回。
      const overrides = JSON.parse(overridesJson) as Record<string, unknown>;
      const handlers: Record<string, (args: Record<string, unknown>) => unknown> =
        {};
      for (const [cmd, returnValue] of Object.entries(overrides)) {
        handlers[cmd] = () =>
          typeof structuredClone === "function"
            ? structuredClone(returnValue)
            : JSON.parse(JSON.stringify(returnValue));
      }
      Object.defineProperty(window, "__TAURI_INTERNALS__", {
        configurable: true,
        value: {
          invoke: (cmd: string, args?: Record<string, unknown>) => {
            if (cmd in handlers) {
              return Promise.resolve(handlers[cmd](args ?? {}));
            }
            // 未知命令：返回空值，让 App 的多数查询静默通过
            return Promise.resolve(null);
          },
          transformCallback: (cb: unknown) => cb,
          metadata: { currentWebview: { windowLabel: "main" } },
          plugins: {},
        },
      });
      // Tauri v2 window/event API 的最小桩
      Object.defineProperty(window, "__TAURI_OS_PLUGIN_INTERNALS__", {
        configurable: true,
        value: {},
      });
    },
    { overridesJson: JSON.stringify(overrides) },
  );
}

const emptyStatus = {
  provider: "antigravity_oauth",
  authenticated: false,
  default_account_id: null,
  migration_error: null,
  accounts: [],
};

test.describe("Antigravity provider（GUI 全链路 mock）", () => {
  test("添加供应商面板中可找到 Antigravity 预设并显示 Google 登录区块", async ({
    page,
  }) => {
    installTauriMock(page, {
      auth_get_status: emptyStatus,
      auth_list_accounts: [],
      check_env_conflicts: [],
    });

    await page.goto("/");

    // 等 App 完成基础挂载（任一主区域按钮出现）
    await page.waitForLoadState("networkidle");

    // 打开添加供应商面板（App 内 aria-label = provider.addNewProvider）
    const addButton = page
      .getByRole("button", { name: /添加新供应商|Add New Provider|addNewProvider/i })
      .first();
    await addButton.click({ timeout: 30_000 });

    // 在预设选择器中搜索 Antigravity
    const search = page.getByPlaceholder(/搜索|Search/i).first();
    if (await search.isVisible().catch(() => false)) {
      await search.fill("Antigravity");
    }

    // 预设卡片出现
    const presetCard = page
      .getByText(/Antigravity \(Google\)|Antigravity/i)
      .first();
    await expect(presetCard).toBeVisible({ timeout: 15_000 });
    await presetCard.click();

    // OAuth 区块渲染：未登录状态出现 Google 登录按钮
    await expect(
      page
        .getByRole("button", { name: /使用 Google 登录|Sign in with Google/i })
        .first(),
    ).toBeVisible({ timeout: 15_000 });
  });

  test("已登录时 Antigravity 区块展示账号列表", async ({ page }) => {
    installTauriMock(page, {
      check_env_conflicts: [],
      auth_get_status: {
        provider: "antigravity_oauth",
        authenticated: true,
        default_account_id: "sub-1",
        migration_error: null,
        accounts: [
          {
            id: "sub-1",
            provider: "antigravity_oauth",
            login: "dev@example.com",
            avatar_url: null,
            authenticated_at: 1,
            is_default: true,
            github_domain: "antigravity.google",
            reauth_required: false,
            requires_reauth: false,
          },
        ],
      },
      auth_list_accounts: [
        {
          id: "sub-1",
          provider: "antigravity_oauth",
          login: "dev@example.com",
          avatar_url: null,
          authenticated_at: 1,
          is_default: true,
          github_domain: "antigravity.google",
          reauth_required: false,
          requires_reauth: false,
        },
      ],
    });

    await page.goto("/");
    await page.waitForLoadState("networkidle");

    const addButton = page
      .getByRole("button", { name: /添加新供应商|Add New Provider|addNewProvider/i })
      .first();
    await addButton.click({ timeout: 30_000 });

    const search = page.getByPlaceholder(/搜索|Search/i).first();
    if (await search.isVisible().catch(() => false)) {
      await search.fill("Antigravity");
    }
    const presetCard = page
      .getByText(/Antigravity \(Google\)|Antigravity/i)
      .first();
    await expect(presetCard).toBeVisible({ timeout: 15_000 });
    await presetCard.click();

    // 已登录：账号邮箱与"添加账号或重新登录"按钮可见。
    // 注意：同一 mock 账号也会出现在 Codex/xAI 的隐藏 Select 选项里，
    // 需取可见的那个（Antigravity 区块渲染在最后，用 .last()）。
    await expect(
      page.getByText("dev@example.com").last(),
    ).toBeVisible({ timeout: 15_000 });
    await expect(
      page
        .getByRole("button", { name: /添加账号或重新登录|Add account or re-login/i })
        .first(),
    ).toBeVisible();
  });
});
