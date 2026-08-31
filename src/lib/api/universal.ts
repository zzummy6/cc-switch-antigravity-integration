import { invoke } from "@tauri-apps/api/core";

export interface UniversalModel {
  id: string;
  displayName: string | null;
}

export interface UniversalRouteEntry {
  providerId: string;
  providerName: string;
  labels: string[];
  appType: string;
  wire: string;
  managed: string | null;
  models: UniversalModel[];
}

export interface UniversalStatusResult {
  gateway: string | null;
  running: boolean;
  routes: UniversalRouteEntry[];
  affinity: Record<string, [string, string]>;
  appDefaults: Record<string, string>;
}

export async function getUniversalStatus(): Promise<UniversalStatusResult> {
  return invoke("universal_status");
}

export async function setRouteAlias(
  providerId: string,
  appType: string,
  aliases: string[],
): Promise<void> {
  return invoke("universal_set_route_alias", {
    providerId,
    appType,
    aliases,
  });
}

export async function clearAffinity(sessionId?: string): Promise<void> {
  return invoke("universal_clear_affinity", { sessionId: sessionId ?? null });
}
