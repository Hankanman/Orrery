import { describe, expect, it } from "vitest";
import { buildCalendar, levelFor, weekdayOf } from "./ContributionGraph";

describe("weekdayOf", () => {
  it("maps epoch day 0 (1970-01-01) to Thursday", () => {
    expect(weekdayOf(0)).toBe(4); // 0 = Sunday, so Thursday = 4
  });
  it("advances by one each day and wraps weekly", () => {
    expect(weekdayOf(1)).toBe(5);
    expect(weekdayOf(2)).toBe(6);
    expect(weekdayOf(3)).toBe(0); // Sunday
    expect(weekdayOf(10)).toBe(weekdayOf(3));
  });
});

describe("levelFor", () => {
  it("buckets counts into 0–4", () => {
    expect(levelFor(0)).toBe(0);
    expect(levelFor(1)).toBe(1);
    expect(levelFor(2)).toBe(1);
    expect(levelFor(3)).toBe(2);
    expect(levelFor(5)).toBe(2);
    expect(levelFor(6)).toBe(3);
    expect(levelFor(9)).toBe(3);
    expect(levelFor(10)).toBe(4);
    expect(levelFor(99)).toBe(4);
  });
});

describe("buildCalendar", () => {
  const today = 20_000; // arbitrary epoch day

  it("produces a 53-week grid of 7-day columns", () => {
    const { weeks } = buildCalendar(today, new Map());
    expect(weeks).toHaveLength(53);
    expect(weeks.every((w) => w.length === 7)).toBe(true);
  });

  it("ends on today's week with today as the latest non-future cell", () => {
    const { weeks } = buildCalendar(today, new Map());
    const lastWeek = weeks[weeks.length - 1];
    const days = lastWeek.map((c) => c.day);
    expect(days).toContain(today);
    // Days after today in the current week are flagged future.
    expect(lastWeek.filter((c) => c.day > today).every((c) => c.future)).toBe(true);
    expect(lastWeek.filter((c) => c.day <= today).every((c) => !c.future)).toBe(true);
  });

  it("sums counts within the window and places them on the right day", () => {
    const counts = new Map<number, number>([
      [today, 4],
      [today - 1, 1],
      [today - 400, 9], // outside the ~371-day window → excluded
    ]);
    const { weeks, total } = buildCalendar(today, counts);
    expect(total).toBe(5); // 4 + 1; the out-of-window 9 is dropped
    const flat = weeks.flat();
    expect(flat.find((c) => c.day === today)?.count).toBe(4);
    expect(flat.find((c) => c.day === today - 1)?.count).toBe(1);
  });
});
