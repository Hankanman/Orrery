import { useEffect, useState } from "react";
import { Spinner } from "@/components/Spinner";
import { useScanStatus } from "@/lib/repos-context";

/**
 * Header activity indicator: the brand spinner plus a label for the current
 * background phase. Lives in the header's flexible area, so it never shifts
 * layout — the spacer absorbs its width and the header height is fixed.
 *
 * Revealed only after activity persists ~350ms, so fast warm-cache scans (now
 * tens of ms, fired on every file-watch tick) don't flicker it; genuine waits
 * — cold scans, network fetches, AI summaries — show it.
 */
export function ScanProgress() {
  const { scanning, fetching, enrich, summarize } = useScanStatus();
  const active = scanning || fetching || enrich !== null || summarize !== null;

  const [show, setShow] = useState(false);
  useEffect(() => {
    if (!active) {
      setShow(false);
      return;
    }
    const t = setTimeout(() => setShow(true), 350);
    return () => clearTimeout(t);
  }, [active]);

  if (!show) return null;

  // Priority: scanning → fetching → enrich → summarize.
  let label = "Working…";
  if (scanning) label = "Scanning repositories…";
  else if (fetching) label = "Fetching from remotes…";
  else if (enrich) label = `Refreshing host data · ${enrich.done}/${enrich.total}`;
  else if (summarize) label = `Generating summaries · ${summarize.done}/${summarize.total}`;

  return (
    <div className="orr-activity" role="status" aria-label={label}>
      <Spinner size={16} />
      <span className="t">{label}</span>
    </div>
  );
}
