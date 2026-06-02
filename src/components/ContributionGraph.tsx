import { useEffect, useMemo, useState } from "react";
import { ipc, isTauri, type DayCount } from "@/lib/ipc";

const WEEKS = 53;
const DAYS = 7;
const SECONDS_PER_DAY = 86_400;
const MS_PER_DAY = SECONDS_PER_DAY * 1000;

// Accent opacity per intensity level; level 0 is the empty-cell class.
const LEVEL_PCT = [0, 24, 46, 68, 92];

export interface Cell {
  day: number; // epoch day
  count: number;
  future: boolean; // day hasn't happened yet (trailing cells of the current week)
}

/** Day-of-week for an epoch day, 0 = Sunday. (1970-01-01 was a Thursday = 4.) */
export function weekdayOf(day: number): number {
  return (((day % DAYS) + 4) % DAYS + DAYS) % DAYS;
}

/** Map a commit count to an intensity level 0–4. */
export function levelFor(count: number): number {
  if (count <= 0) return 0;
  if (count <= 2) return 1;
  if (count <= 5) return 2;
  if (count <= 9) return 3;
  return 4;
}

/** The local epoch day for a timestamp (defaults to now). */
export function localEpochDay(nowMs: number): number {
  const offsetMin = new Date(nowMs).getTimezoneOffset();
  return Math.floor((nowMs / 1000 - offsetMin * 60) / SECONDS_PER_DAY);
}

/**
 * Build the trailing-53-week calendar ending on `today`: columns are weeks
 * (oldest first), rows are days (Sunday top). Returns the week columns and the
 * total commits in the window.
 */
export function buildCalendar(today: number, counts: Map<number, number>): { weeks: Cell[][]; total: number } {
  const lastSunday = today - weekdayOf(today);
  const firstSunday = lastSunday - DAYS * (WEEKS - 1);
  const weeks: Cell[][] = [];
  let total = 0;
  for (let w = 0; w < WEEKS; w++) {
    const col: Cell[] = [];
    for (let d = 0; d < DAYS; d++) {
      const day = firstSunday + w * DAYS + d;
      const count = counts.get(day) ?? 0;
      if (day <= today) total += count;
      col.push({ day, count, future: day > today });
    }
    weeks.push(col);
  }
  return { weeks, total };
}

const utcDate = (day: number) => new Date(day * MS_PER_DAY);
const monthShort = (day: number) => utcDate(day).toLocaleDateString(undefined, { timeZone: "UTC", month: "short" });
const fullDate = (day: number) =>
  utcDate(day).toLocaleDateString(undefined, { timeZone: "UTC", weekday: "short", month: "short", day: "numeric", year: "numeric" });

interface ContributionGraphProps {
  /** Repo ids (absolute paths) to aggregate commits across. */
  ids: string[];
}

export function ContributionGraph({ ids }: ContributionGraphProps) {
  const [data, setData] = useState<DayCount[] | null>(null);
  const key = ids.join("|");

  useEffect(() => {
    if (!isTauri() || ids.length === 0) {
      setData([]);
      return;
    }
    let live = true;
    ipc
      .contributionGraph(ids)
      .then((d) => live && setData(d))
      .catch(() => live && setData([]));
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  const { weeks, total } = useMemo(() => {
    const counts = new Map<number, number>();
    for (const d of data ?? []) counts.set(d.day, d.count);
    return buildCalendar(localEpochDay(Date.now()), counts);
  }, [data]);

  // Don't occupy space in the browser dev preview where there's no git data.
  if (!isTauri()) return null;

  // Month labels: show the month at the column where it first appears.
  let prevMonth = "";

  return (
    <section className="orr-contrib" aria-label="Commit contribution graph">
      <header className="orr-contrib-head">
        <span className="t">Activity</span>
        <span className="n">
          {data === null ? "…" : `${total.toLocaleString()} ${total === 1 ? "commit" : "commits"} in the last year`}
        </span>
        <span className="legend">
          Less
          {[0, 1, 2, 3, 4].map((l) => (
            <i key={l} className={`day lvl-${l}`} style={cellStyle(l)} aria-hidden />
          ))}
          More
        </span>
      </header>

      <div className="orr-contrib-cal">
        <div className="months" aria-hidden>
          {weeks.map((col) => {
            const m = monthShort(col[0].day);
            const label = m === prevMonth ? "" : ((prevMonth = m), m);
            return (
              <span className="m" key={col[0].day}>
                {label}
              </span>
            );
          })}
        </div>
        <div className="weeks">
          {weeks.map((col) => (
            <div className="wk" key={col[0].day}>
              {col.map((cell) => {
                const level = levelFor(cell.count);
                return (
                  <i
                    key={cell.day}
                    className={`day lvl-${level}${cell.future ? " future" : ""}`}
                    style={cell.future ? undefined : cellStyle(level)}
                    title={cell.future ? undefined : `${cell.count} ${cell.count === 1 ? "commit" : "commits"} · ${fullDate(cell.day)}`}
                  />
                );
              })}
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}

function cellStyle(level: number): React.CSSProperties | undefined {
  if (level === 0) return undefined; // empty cell handled by .lvl-0 class
  return { background: `color-mix(in srgb, var(--primary) ${LEVEL_PCT[level]}%, transparent)` };
}
