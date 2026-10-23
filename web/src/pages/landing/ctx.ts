import { createContext } from "preact";
import { useContext } from "preact/hooks";
import type { Store } from "./store";

export const StoreCtx = createContext<Store | null>(null);

export function useStore(): Store {
  const s = useContext(StoreCtx);
  if (!s) throw new Error("StoreCtx 缺失");
  return s;
}
