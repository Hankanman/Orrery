import { useRef, type ReactNode } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { cn } from "@/lib/utils";

interface VirtualListProps<T> {
  items: T[];
  getKey: (item: T) => string;
  renderItem: (item: T) => ReactNode;
  /** Estimated row height (px) before measurement. */
  estimateSize?: number;
  /** Vertical gap between rows (px). */
  gap?: number;
  overscan?: number;
  /** Applied to the scroll container — set a max-height + any padding here. */
  className?: string;
}

/**
 * Generic windowed vertical list: only the visible rows are in the DOM, so it
 * stays smooth with hundreds/thousands of items. Rows are measured dynamically,
 * so variable heights are fine. The container scrolls within whatever bound the
 * `className` gives it (e.g. `max-h-80`).
 */
export function VirtualList<T>({
  items,
  getKey,
  renderItem,
  estimateSize = 36,
  gap = 0,
  overscan = 8,
  className,
}: VirtualListProps<T>) {
  const ref = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => ref.current,
    estimateSize: () => estimateSize,
    overscan,
    gap,
  });

  return (
    <div ref={ref} className={cn("overflow-y-auto", className)}>
      <div style={{ position: "relative", width: "100%", height: virtualizer.getTotalSize() }}>
        {virtualizer.getVirtualItems().map((row) => (
          <div
            key={getKey(items[row.index])}
            ref={virtualizer.measureElement}
            data-index={row.index}
            style={{ position: "absolute", top: 0, left: 0, width: "100%", transform: `translateY(${row.start}px)` }}
          >
            {renderItem(items[row.index])}
          </div>
        ))}
      </div>
    </div>
  );
}
