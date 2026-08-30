import React from "react";
import { useTranslation } from "react-i18next";
import {
  AlertTriangle,
  ExternalLink,
  Loader2,
  LogOut,
  Plus,
  Sparkles,
  User,
  X,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useAntigravityOauth } from "./hooks/useAntigravityOauth";
import { testAntigravityConnection } from "@/lib/api/auth";

interface AntigravityOAuthSectionProps {
  className?: string;
  selectedAccountId?: string | null;
  onAccountSelect?: (accountId: string | null) => void;
}

/**
 * Antigravity (Google Cloud Code) OAuth section.
 *
 * Unlike the device-flow providers there is no user code to copy: the backend
 * binds a localhost loopback server and the browser returns directly to it,
 * so the polling UI only shows "waiting for consent" with the auth URL.
 */
export const AntigravityOAuthSection: React.FC<AntigravityOAuthSectionProps> = ({
  className,
  selectedAccountId,
  onAccountSelect,
}) => {
  const { t } = useTranslation();
  const {
    accounts,
    defaultAccountId,
    hasAnyAccount,
    isAuthenticated,
    pollingState,
    deviceCode,
    error,
    isPolling,
    isAddingAccount,
    isRemovingAccount,
    isSettingDefaultAccount,
    addAccount,
    removeAccount,
    setDefaultAccount,
    cancelAuth,
    logout,
  } = useAntigravityOauth();

  const usableAccounts = accounts.filter((account) => !account.requires_reauth);

  const [testState, setTestState] = React.useState<
    | { kind: "idle" }
    | { kind: "running" }
    | { kind: "ok"; latencyMs: number; sampleReply: string }
    | { kind: "error"; message: string }
  >({ kind: "idle" });
  const runConnectionTest = async () => {
    setTestState({ kind: "running" });
    try {
      const result = await testAntigravityConnection(
        selectedAccountId ?? defaultAccountId,
      );
      setTestState({
        kind: "ok",
        latencyMs: result.latencyMs,
        sampleReply: result.sampleReply,
      });
    } catch (error) {
      setTestState({ kind: "error", message: String(error) });
    }
  };

  const remove = (accountId: string, event: React.MouseEvent) => {
    event.preventDefault();
    event.stopPropagation();
    removeAccount(accountId);
    if (selectedAccountId === accountId) onAccountSelect?.(null);
  };

  return (
    <div className={`space-y-4 ${className ?? ""}`}>
      {isAuthenticated && (
        <div className="rounded-lg border border-green-500/40 bg-green-500/5 p-3">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2 text-sm">
              <span className="relative flex h-2.5 w-2.5">
                <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-green-400 opacity-75" />
                <span className="relative inline-flex h-2.5 w-2.5 rounded-full bg-green-500" />
              </span>
              <span className="font-medium">Connected</span>
              <span className="truncate text-muted-foreground">
                {defaultAccountId
                  ? (accounts.find((a) => a.id === defaultAccountId)?.login ??
                    "")
                  : ""}
              </span>
            </div>
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={testState.kind === "running"}
              onClick={runConnectionTest}
            >
              {testState.kind === "running"
                ? t("antigravityOauth.testing", "测试中…")
                : t("antigravityOauth.test", "Test")}
            </Button>
          </div>
          {testState.kind === "ok" && (
            <p className="mt-2 text-xs text-green-600">
              {t("antigravityOauth.testOk", {
                latency: testState.latencyMs,
                reply: testState.sampleReply,
                defaultValue: `连通正常 · ${testState.latencyMs}ms · 回复「${testState.sampleReply}」`,
              })}
            </p>
          )}
          {testState.kind === "error" && (
            <p className="mt-2 text-xs text-red-500">{testState.message}</p>
          )}
        </div>
      )}
      <div className="flex items-center justify-between">
        <Label>
          {t("antigravityOauth.authStatus", "Antigravity OAuth 认证")}
        </Label>
        <Badge
          variant={isAuthenticated ? "default" : "secondary"}
          className={
            isAuthenticated
              ? "bg-green-500 hover:bg-green-600"
              : hasAnyAccount
                ? "border-amber-500 text-amber-600"
                : ""
          }
        >
          {isAuthenticated
            ? t("antigravityOauth.accountCount", {
                count: usableAccounts.length,
                defaultValue: `${usableAccounts.length} 个可用账号`,
              })
            : hasAnyAccount
              ? t("antigravityOauth.reauthRequired", "需要重新登录")
              : t("antigravityOauth.notAuthenticated", "未认证")}
        </Badge>
      </div>

      {accounts.length > 0 && onAccountSelect && (
        <div className="space-y-2">
          <Label className="text-sm text-muted-foreground">
            {t("antigravityOauth.selectAccount", "选择账号")}
          </Label>
          <Select
            value={selectedAccountId || "none"}
            onValueChange={(value) =>
              onAccountSelect(value === "none" ? null : value)
            }
          >
            <SelectTrigger>
              <SelectValue
                placeholder={t(
                  "antigravityOauth.selectAccountPlaceholder",
                  "选择 Google 账号",
                )}
              />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="none">
                {t("antigravityOauth.useDefaultAccount", "使用默认账号")}
              </SelectItem>
              {accounts.map((account) => (
                <SelectItem
                  key={account.id}
                  value={account.id}
                  disabled={account.requires_reauth}
                >
                  <span className="flex items-center gap-2">
                    {account.requires_reauth ? (
                      <AlertTriangle className="h-4 w-4 text-amber-500" />
                    ) : (
                      <User className="h-4 w-4 text-muted-foreground" />
                    )}
                    {account.login}
                    {account.requires_reauth && (
                      <span className="text-xs text-amber-600">
                        ({t("antigravityOauth.expired", "凭据已失效")})
                      </span>
                    )}
                  </span>
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      )}

      {hasAnyAccount && (
        <div className="space-y-2">
          <Label className="text-sm text-muted-foreground">
            {t("antigravityOauth.accounts", "Google 账号")}
          </Label>
          <div className="space-y-1">
            {accounts.map((account) => (
              <div
                key={account.id}
                className="flex items-center justify-between rounded-md border bg-muted/30 p-2"
              >
                <div className="flex min-w-0 items-center gap-2">
                  {account.requires_reauth ? (
                    <AlertTriangle className="h-5 w-5 shrink-0 text-amber-500" />
                  ) : (
                    <User className="h-5 w-5 shrink-0 text-muted-foreground" />
                  )}
                  <span className="truncate text-sm font-medium">
                    {account.login}
                  </span>
                  {defaultAccountId === account.id && (
                    <Badge variant="secondary" className="text-xs">
                      {t("antigravityOauth.defaultAccount", "默认")}
                    </Badge>
                  )}
                  {account.requires_reauth && (
                    <Badge
                      variant="outline"
                      className="border-amber-500 text-xs text-amber-600"
                    >
                      {t("antigravityOauth.expired", "凭据已失效")}
                    </Badge>
                  )}
                </div>
                <div className="flex items-center gap-1">
                  {!account.requires_reauth &&
                    defaultAccountId !== account.id && (
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        className="h-7 px-2 text-xs"
                        disabled={isSettingDefaultAccount}
                        onClick={() => setDefaultAccount(account.id)}
                      >
                        {t("antigravityOauth.setAsDefault", "设为默认")}
                      </Button>
                    )}
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="h-7 w-7 text-muted-foreground hover:text-red-500"
                    disabled={isRemovingAccount}
                    onClick={(event) => remove(account.id, event)}
                    title={t("antigravityOauth.removeAccount", "移除账号")}
                  >
                    <X className="h-4 w-4" />
                  </Button>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {pollingState === "idle" && (
        <Button
          type="button"
          variant="outline"
          className="w-full"
          disabled={isAddingAccount}
          onClick={addAccount}
        >
          {hasAnyAccount ? (
            <Plus className="mr-2 h-4 w-4" />
          ) : (
            <Sparkles className="mr-2 h-4 w-4" />
          )}
          {hasAnyAccount
            ? t("antigravityOauth.addOrReauth", "添加账号或重新登录")
            : t("antigravityOauth.login", "使用 Google 登录")}
        </Button>
      )}

      {isPolling && (
        <div className="space-y-3 rounded-lg border bg-muted/50 p-4">
          <div className="flex items-center justify-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" />
            {t(
              "antigravityOauth.waitingForAuth",
              "等待 Google 授权完成…（浏览器窗口中登录）",
            )}
          </div>
          {deviceCode?.verification_uri && (
            <div className="text-center">
              <a
                href={deviceCode.verification_uri}
                target="_blank"
                rel="noopener noreferrer"
                className="inline-flex items-center gap-1 text-sm text-blue-500 hover:underline"
              >
                {t("antigravityOauth.openAuthPage", "重新打开授权页面")}
                <ExternalLink className="h-3 w-3" />
              </a>
            </div>
          )}
          <div className="text-center">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={cancelAuth}
            >
              {t("common.cancel", "取消")}
            </Button>
          </div>
        </div>
      )}

      {pollingState === "error" && error && (
        <div className="space-y-2">
          <p className="text-sm text-red-500">{error}</p>
          <div className="flex gap-2">
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={addAccount}
            >
              {t("antigravityOauth.retry", "重试")}
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={cancelAuth}
            >
              {t("common.cancel", "取消")}
            </Button>
          </div>
        </div>
      )}

      {hasAnyAccount && accounts.length > 1 && (
        <Button
          type="button"
          variant="outline"
          className="w-full text-red-500 hover:text-red-600"
          onClick={logout}
        >
          <LogOut className="mr-2 h-4 w-4" />
          {t("antigravityOauth.logoutAll", "移除所有 Antigravity 账号")}
        </Button>
      )}
    </div>
  );
};

export default AntigravityOAuthSection;
