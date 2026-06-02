import { createContext, useContext, useEffect, useState, type ReactNode } from "react";

// The persistent sidebar shows a fixed primary nav plus a per-screen "slot".
// Each page fills the slot with its own contextual content (grid facets,
// settings sections, …). Setter and value live in separate contexts so a page
// that only *sets* the slot doesn't re-render when the slot value changes.
const SetSlotContext = createContext<(node: ReactNode) => void>(() => {});
const SlotContext = createContext<ReactNode>(null);

export function SidebarSlotProvider({ children }: { children: ReactNode }) {
  const [slot, setSlot] = useState<ReactNode>(null);
  return (
    <SetSlotContext.Provider value={setSlot}>
      <SlotContext.Provider value={slot}>{children}</SlotContext.Provider>
    </SetSlotContext.Provider>
  );
}

/** Read the current slot content (the persistent sidebar consumes this). */
export function useSidebarSlotValue(): ReactNode {
  return useContext(SlotContext);
}

/**
 * Render `node` into the sidebar's contextual slot while the calling page is
 * mounted. Pass a memoized node so it only updates when its inputs change.
 */
export function useSidebarSlot(node: ReactNode): void {
  const setSlot = useContext(SetSlotContext);
  useEffect(() => {
    setSlot(node);
    return () => setSlot(null);
  }, [setSlot, node]);
}
