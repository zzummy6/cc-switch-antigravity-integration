import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Activity, RefreshCw, Route, Trash2 } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { universalApi } from "@/lib/api";
import type { UniversalRouteEntry } from "@/lib/api";

const WIRE_BADGE_COLORS: Record<string, string> = {
  anthropic: "bg-orange-500/15 text-orange-600 border-orange-500/30",
  openai_chat: "bg-emerald-500/15 text-emerald-600 border-emerald-500/30",
  openai_responses: "bg-teal-500/15 text-teal-600 border-teal-500/30",
  gemini_native: "bg-sky-500/15 text-sky-600 border-sky-500/30",
};

function RouteRow({ route }: { route: UniversalRouteEntry }) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(route.labels.join(", "));
  const queryClient = useQueryClient();

  const saveAlias = useMutation({
    mutationFn: () =>
      universalApi.setRouteAlias(
        route.providerId,
        route.appType,
        draft.split(",").map((s) => s.trim()).filter(Boolean),
      ),
    onSuccess: () => {
      setEditing(false);
      queryClient.invalidateQueries({ queryKey: ["universal-status"] });
    },
  });

  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1 rounded-md border bg-muted/20 px-3 py-2 text-sm">
      <span className="font-medium">{route.providerName}</span>
      <Badge variant="outline" className="font-mono text-xs">
        {route.appType}
      </Badge>
      <Badge
        variant="outline"
        className={`text-xs ${WIRE_BADGE_COLORS[route.wire] ?? ""}`}
      >
        {route.wire}
      </Badge>
      {route.managed && (
        <Badge variant="secondary" className="text-xs">
          {t("universalGateway.managed", "OAuth 托管")}
        </Badge>
      )}
      <span className="text-xs text-muted-foreground">
        {t("universalGateway.requestWith", "请求写法")}
      </span>
      {editing ? (
        <span className="flex items-center gap-1">
          <Input
            className="h-7 w-44 font-mono text-xs"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder="ag, antigravity, …"
          />
          <Button
            size="sm"
            variant="ghost"
            className="h-7 text-xs"
            disabled={saveAlias.isPending}
            onClick={() => saveAlias.mutate()}
          >
            {t("common.save", "保存")}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            className="h-7 text-xs"
            onClick={() => {
              setDraft(route.labels.join(", "));
              setEditing(false);
            }}
          >
            {t("common.cancel", "取消")}
          </Button>
        </span>
      ) : (
        <span className="flex items-center gap-1">
          <code className="rounded bg-background px-1.5 py-0.5 font-mono text-xs">
            {route.labels.slice(0, 3).join(" | ")}/
            {route.models[0]?.id ?? "model"}
          </code>
          <Button
            size="icon"
            variant="ghost"
            className="h-6 w-6 text-muted-foreground"
            title={t("universalGateway.editAlias", "编辑路由别名")}
            onClick={() => setEditing(true)}
          >
            <RefreshCw className="h-3 w-3" />
          </Button>
        </span>
      )}
      {route.models.length > 0 && (
        <span
          className="truncate text-xs text-muted-foreground"
          title={route.models.map((m) => m.id).join(", ")}
        >
          {route.models.length} {t("universalGateway.models", "模型")}
        </span>
      )}
    </div>
  );
}

export function UniversalGatewayPanel() {
  const { t } = useTranslation();

  const { data, isLoading, refetch } = useQuery({
    queryKey: ["universal-status"],
    queryFn: () => universalApi.getUniversalStatus(),
    refetchInterval: 15_000,
  });

  const clearAffinity = useMutation({
    mutationFn: () => universalApi.clearAffinity(),
    onSuccess: () => refetch(),
  });

  const affinityEntries = Object.entries(data?.affinity ?? {});

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2 text-base">
          <Route className="h-4 w-4" />
          {t("universalGateway.title", "Universal Gateway（请求级路由）")}
        </CardTitle>
        <CardDescription>
          {t(
            "universalGateway.description",
            "所有客户端共用同一网关端口。在 model 字段使用 “provider/模型名” 即可实时切换上游供应商，无需在界面切换当前供应商。",
          )}
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="flex items-center gap-2 text-sm">
          <Activity
            className={`h-4 w-4 ${data?.running ? "text-green-500" : "text-muted-foreground"}`}
          />
          <span>
            {data?.running
              ? t("universalGateway.running", "运行中")
              : t("universalGateway.stopped", "未运行")}
          </span>
          {data?.gateway && (
            <code className="rounded bg-muted px-1.5 py-0.5 font-mono text-xs">
              {data.gateway}
            </code>
          )}
          <Button
            size="sm"
            variant="ghost"
            onClick={() => refetch()}
            title={t("common.refresh", "刷新")}
          >
            <RefreshCw className="h-3.5 w-3.5" />
          </Button>
        </div>

        {data && Object.keys(data.appDefaults).length > 0 && (
          <div className="text-xs text-muted-foreground">
            {t("universalGateway.fallbackNote", "未带前缀且注册表未命中的请求回退到：")}{" "}
            {Object.entries(data.appDefaults)
              .map(([app, name]) => `${app}=${name}`)
              .join("，")}
          </div>
        )}

        {isLoading ? (
          <div className="py-4 text-center text-sm text-muted-foreground">
            …
          </div>
        ) : (
          <div className="space-y-2">
            {(data?.routes ?? []).map((route) => (
              <RouteRow key={`${route.appType}:${route.providerId}`} route={route} />
            ))}
            {(data?.routes.length ?? 0) === 0 && (
              <p className="text-sm text-muted-foreground">
                {t(
                  "universalGateway.noRoutes",
                  "暂无可路由的供应商（为供应商配置地址与模型后可用）。",
                )}
              </p>
            )}
          </div>
        )}

        <div className="space-y-2 border-t pt-3">
          <div className="flex items-center justify-between">
            <h4 className="text-sm font-medium">
              {t("universalGateway.activeSessions", "活跃会话亲和")}
              <span className="ml-2 text-xs text-muted-foreground">
                {affinityEntries.length}
              </span>
            </h4>
            {affinityEntries.length > 0 && (
              <Button
                size="sm"
                variant="ghost"
                className="text-red-500 hover:text-red-600"
                disabled={clearAffinity.isPending}
                onClick={() => clearAffinity.mutate()}
              >
                <Trash2 className="mr-1 h-3 w-3" />
                {t("universalGateway.clearAffinity", "清除全部")}
              </Button>
            )}
          </div>
          {affinityEntries.length === 0 ? (
            <p className="text-xs text-muted-foreground">
              {t(
                "universalGateway.noAffinity",
                "暂无记录。带 session 的路由请求会在此显示会话→供应商粘滞。",
              )}
            </p>
          ) : (
            affinityEntries.slice(0, 20).map(([session, [label, model]]) => (
              <div
                key={session}
                className="flex items-center justify-between rounded-md border bg-muted/20 px-2 py-1 text-xs"
              >
                <code className="truncate font-mono">{session.slice(0, 20)}…</code>
                <code className="ml-2 shrink-0 rounded bg-background px-1 py-0.5 font-mono">
                  {label}/{model}
                </code>
              </div>
            ))
          )}
        </div>
      </CardContent>
    </Card>
  );
}
