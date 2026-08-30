import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AntigravityOAuthSection } from "@/components/providers/forms/AntigravityOAuthSection";
import { AuthCenterPanel } from "@/components/settings/AuthCenterPanel";

const mocks = vi.hoisted(() => ({
  useAntigravityOauth: vi.fn(),
}));

vi.mock("@/components/providers/forms/hooks/useAntigravityOauth", () => ({
  useAntigravityOauth: mocks.useAntigravityOauth,
}));

vi.mock("@/components/providers/forms/CopilotAuthSection", () => ({
  CopilotAuthSection: () => <div />,
}));

vi.mock("@/components/providers/forms/CodexOAuthSection", () => ({
  CodexOAuthSection: () => <div />,
}));

vi.mock("@/components/providers/forms/XaiOAuthSection", () => ({
  XaiOAuthSection: () => <div />,
}));

const baseHookState = {
  accounts: [] as Array<{
    id: string;
    provider: string;
    login: string;
    avatar_url: string | null;
    authenticated_at: number;
    is_default: boolean;
    github_domain: string;
    reauth_required: boolean;
    requires_reauth: boolean;
  }>,
  defaultAccountId: null as string | null,
  isStatusSuccess: true,
  isStatusError: false,
  hasAnyAccount: false,
  pollingState: "idle" as "idle" | "polling" | "success" | "error",
  deviceCode: null as null | {
    device_code: string;
    user_code: string;
    verification_uri: string;
    expires_in: number;
    interval: number;
  },
  error: null as string | null,
  isPolling: false,
  isAddingAccount: false,
  isRemovingAccount: false,
  isSettingDefaultAccount: false,
  addAccount: vi.fn(),
  removeAccount: vi.fn(),
  setDefaultAccount: vi.fn(),
  cancelAuth: vi.fn(),
  logout: vi.fn(),
};

describe("AntigravityOAuthSection", () => {
  let scrollIntoViewDescriptor: PropertyDescriptor | undefined;

  beforeEach(() => {
    scrollIntoViewDescriptor = Object.getOwnPropertyDescriptor(
      HTMLElement.prototype,
      "scrollIntoView",
    );
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
      configurable: true,
      value: vi.fn(),
    });
    mocks.useAntigravityOauth.mockReturnValue({ ...baseHookState });
  });

  afterEach(() => {
    if (scrollIntoViewDescriptor) {
      Object.defineProperty(
        HTMLElement.prototype,
        "scrollIntoView",
        scrollIntoViewDescriptor,
      );
    }
    vi.clearAllMocks();
  });

  it("未登录时展示 Google 登录按钮（无 user_code 复制框）", async () => {
    render(<AntigravityOAuthSection />);
    await userEvent.click(
      await screen.findByRole("button", { name: /Google 登录/ }),
    );
    expect(baseHookState.addAccount).toHaveBeenCalledTimes(1);
    expect(screen.queryByText(/请输入/)).not.toBeInTheDocument();
  });

  it("展示账号列表并允许设为默认/移除", async () => {
    mocks.useAntigravityOauth.mockReturnValue({
      ...baseHookState,
      hasAnyAccount: true,
      isAuthenticated: true,
      defaultAccountId: "sub-1",
      accounts: [
        {
          id: "sub-1",
          provider: "antigravity_oauth",
          login: "one@example.com",
          avatar_url: null,
          authenticated_at: 1,
          is_default: true,
          github_domain: "antigravity.google",
          reauth_required: false,
          requires_reauth: false,
        },
        {
          id: "sub-2",
          provider: "antigravity_oauth",
          login: "two@example.com",
          avatar_url: null,
          authenticated_at: 2,
          is_default: false,
          github_domain: "antigravity.google",
          reauth_required: false,
          requires_reauth: false,
        },
      ],
    });

    render(<AntigravityOAuthSection />);
    // Connected 状态卡与账号列表都会显示默认账号邮箱
    expect(
      (await screen.findAllByText("one@example.com")).length,
    ).toBeGreaterThanOrEqual(1);
    expect(screen.getByText("two@example.com")).toBeInTheDocument();

    await userEvent.click(
      screen.getByRole("button", { name: /设为默认/ }),
    );
    expect(baseHookState.setDefaultAccount).toHaveBeenCalledWith("sub-2");
  });

  it("登录轮询中显示浏览器授权提示（而非设备码）", async () => {
    mocks.useAntigravityOauth.mockReturnValue({
      ...baseHookState,
      pollingState: "polling",
      isPolling: true,
      deviceCode: {
        device_code: "session-uuid",
        user_code: "",
        verification_uri:
          "https://accounts.google.com/o/oauth2/v2/auth?client_id=test",
        expires_in: 600,
        interval: 2,
      },
    });

    render(<AntigravityOAuthSection />);
    expect(
      await screen.findByText(/等待 Google 授权完成/),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: /重新打开授权页面/ }),
    ).toHaveAttribute(
      "href",
      "https://accounts.google.com/o/oauth2/v2/auth?client_id=test",
    );
  });

  it("挂载在 AuthCenterPanel 并可通过锚点滚动定位", async () => {
    render(
      <AuthCenterPanel authScrollTarget={"antigravity_oauth" as never} />,
    );
    await waitFor(() => {
      expect(
        screen.getAllByText(/Antigravity/).length,
      ).toBeGreaterThan(0);
    });
  });
});
