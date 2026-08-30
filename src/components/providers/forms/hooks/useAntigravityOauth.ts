import { useManagedAuth } from "./useManagedAuth";

/** Antigravity (Google Cloud Code) OAuth hook — browser consent + loopback. */
export function useAntigravityOauth() {
  return useManagedAuth("antigravity_oauth");
}
