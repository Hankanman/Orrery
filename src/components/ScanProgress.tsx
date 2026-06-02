import { useEffect, useState } from "react";
import { useScanStatus } from "@/lib/repos-context";

/**
 * Thin global activity bar under the header. Indeterminate while scanning or
 * fetching, determinate (with a count) while enriching host data or generating
 * AI summaries.
 *
 * Only revealed once activity has persisted past a short delay, so the common
 * fast warm-cache scan (≈tens of ms, fired on every file-watch tick) doesn't
 * flash the bar in and out. Genuine waits — cold scans, network fetches, AI
 * summaries — cross the threshold and show feedback.
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
  let label: string;
  let pct: number | null; // null = indeterminate
  if (scanning) {
    label = "Scanning repositories";
    pct = null;
  } else if (fetching) {
    label = "Fetching from remotes";
    pct = null;
  } else if (enrich) {
    label = `Refreshing host data ${enrich.done}/${enrich.total}`;
    pct = enrich.total ? (enrich.done / enrich.total) * 100 : null;
  } else if (summarize) {
    label = `Generating summaries ${summarize.done}/${summarize.total}`;
    pct = summarize.total ? (summarize.done / summarize.total) * 100 : null;
  } else {
    return null;
  }

  return (
    <div className="orr-progress" role="status" aria-label={label}>
      <div className={`bar${pct === null ? " indeterminate" : ""}`} style={pct === null ? undefined : { width: `${pct}%` }} />
      <span className="lbl">{label}</span>
    </div>
  );
}
