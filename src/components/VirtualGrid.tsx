import { useEffect, useRef, useState, type ReactNode } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { cn } from "@/lib/utils";

interface VirtualGridProps<T> {
  items: T[];
  /** Return a *keyed* element for an item. */
  renderItem: (item: T) => ReactNode;
  /** Min item width (px) for responsive columns. Ignored when `columns` is set. */
  minColWidth?: number;
  /** Force a fixed column count (e.g. 1 for a list). */
  columns?: number;
  colGap?: number;
  rowGap?: number;
  /** Estimated row height (px) before measurement. */
  estimateRow?: number;
  overscan?: number;
  /** Applied to the scroll container — set padding and any max-height here. */
  className?: string;
}

/**
 * Windowed responsive grid: only visible rows are in the DOM. Columns are
 * derived from the container width (or fixed via `columns`); rows are measured
 * dynamically so variable item heights work. Reused by the repo grid and the
 * starred browser.
 */
export function VirtualGrid<T>({
  items,
  renderItem,
  minColWidth = 320,
  columns: forced,
  colGap = 14,
  rowGap = 14,
  estimateRow = 200,
  overscan = 6,
  className,
}: VirtualGridProps<T>) {
  const parentRef = useRef<HTMLDivElement>(null);
  const [columns, setColumns] = useState(forced ?? 1);

  useEffect(() => {
    if (forced) {
      setColumns(forced);
      return;
    }
    const el = parentRef.current;
    if (!el) return;
    const compute = () => {
      const s = getComputedStyle(el);
      const padX = parseFloat(s.paddingLeft) + parseFloat(s.paddingRight);
      const inner = el.clientWidth - padX;
      setColumns(Math.max(1, Math.floor((inner + colGap) / (minColWidth + colGap))));
    };
    compute();
    const ro = new ResizeObserver(compute);
    ro.observe(el);
    return () => ro.disconnect();
  }, [forced, minColWidth, colGap]);

  const rowCount = Math.ceil(items.length / columns);
  const virtualizer = useVirtualizer({
    count: rowCount,
    getScrollElement: () => parentRef.current,
    estimateSize: () => estimateRow,
    overscan,
    gap: rowGap,
  });

  return (
    <div ref={parentRef} className={cn("overflow-y-auto", className)}>
      <div style={{ position: "relative", width: "100%", height: virtualizer.getTotalSize() }}>
        {virtualizer.getVirtualItems().map((row) => {
          const start = row.index * columns;
          const rowItems = items.slice(start, start + columns);
          return (
            <div
              key={row.key}
              ref={virtualizer.measureElement}
              data-index={row.index}
              style={{
                position: "absolute",
                top: 0,
                left: 0,
                width: "100%",
                transform: `translateY(${row.start}px)`,
                display: "grid",
                gridTemplateColumns: columns === 1 ? "1fr" : `repeat(${columns}, minmax(0, 1fr))`,
                columnGap: colGap,
                alignItems: "start",
              }}
            >
              {rowItems.map((item) => renderItem(item))}
            </div>
          );
        })}
      </div>
    </div>
  );
}
