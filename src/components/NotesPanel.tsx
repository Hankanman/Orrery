import { useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Check, History, Sparkles } from "lucide-react";
import { ipc, isTauri, type ResumeSummary } from "@/lib/ipc";
import { cn } from "@/lib/utils";

const SAVE_DEBOUNCE_MS = 800;

/** Per-repo markdown scratchpad + a "what changed since you last looked" panel. */
export function NotesPanel({ id, aiReady }: { id: string; aiReady: boolean }) {
  const [note, setNote] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [saved, setSaved] = useState(true);
  const [resume, setResume] = useState<ResumeSummary | null>(null);
  const [resumeBusy, setResumeBusy] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Latest text + unsaved flag, read by the unmount flush without re-subscribing.
  const noteRef = useRef("");
  const dirty = useRef(false);

  // Load the note, then compute the catch-up against the stored cursor and mark
  // the repo as seen — so the next open measures from here. Runs once per repo.
  useEffect(() => {
    let alive = true;
    setNote("");
    noteRef.current = "";
    dirty.current = false;
    setLoaded(false);
    setSaved(true);
    setResume(null);
    if (!isTauri()) {
      setLoaded(true);
      return;
    }
    ipc
      .getNote(id)
      .then((t) => {
        if (!alive) return;
        setNote(t);
        noteRef.current = t;
        setLoaded(true);
      })
      .catch(() => alive && setLoaded(true));
    setResumeBusy(true);
    ipc
      .resumeSummary(id)
      .then((r) => {
        if (alive) setResume(r);
        // Advance the cursor once we've captured what changed.
        return ipc.markSeen(id).catch(() => {});
      })
      .catch(() => {})
      .finally(() => alive && setResumeBusy(false));
    return () => {
      alive = false;
    };
  }, [id]);

  // Debounced persistence. The flush on cleanup runs only on unmount / repo
  // change (keyed on `id`), reading the latest text from a ref — so a quick tab
  // switch never drops the last keystrokes, but typing isn't saved per-keystroke.
  const flush = (text: string) => {
    dirty.current = false;
    if (isTauri()) ipc.setNote(id, text).then(() => setSaved(true)).catch(() => {});
  };
  useEffect(() => {
    return () => {
      if (timer.current) clearTimeout(timer.current);
      if (dirty.current) flush(noteRef.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  const onChange = (text: string) => {
    setNote(text);
    noteRef.current = text;
    dirty.current = true;
    setSaved(false);
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => flush(text), SAVE_DEBOUNCE_MS);
  };

  const showResume = resumeBusy || (resume && !resume.firstVisit && resume.commitCount > 0);

  return (
    <div className="space-y-4">
      {showResume && (
        <section className="rounded-md border border-border bg-background/40 p-3">
          <div className="mb-1.5 flex items-center gap-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            <History className="size-3.5" /> Since you last looked
          </div>
          {resumeBusy ? (
            <p className="flex items-center gap-2 text-sm text-muted-foreground">
              {aiReady && <Sparkles className="size-3.5 animate-pulse" />} Catching you up…
            </p>
          ) : (
            resume && (
              <div className="space-y-1.5">
                <p className="text-sm font-medium">
                  {resume.commitCount} commit{resume.commitCount === 1 ? "" : "s"} since your last visit
                </p>
                {resume.text ? (
                  <p className="text-sm text-muted-foreground">{resume.text}</p>
                ) : (
                  aiReady && <p className="text-xs text-muted-foreground">No summary available.</p>
                )}
              </div>
            )
          )}
        </section>
      )}

      <section>
        <div className="mb-2 flex items-center justify-between">
          <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">Notes</h3>
          <span className={cn("flex items-center gap-1 text-xs text-muted-foreground", saved ? "opacity-100" : "opacity-0")}>
            <Check className="size-3" /> Saved
          </span>
        </div>
        <textarea
          className="min-h-[40vh] w-full resize-y rounded-md border border-border bg-background/60 p-3 font-mono text-sm leading-relaxed outline-none focus:border-primary/50"
          placeholder={loaded ? "Jot context, TODOs, where you left off… (markdown, autosaves)" : "Loading…"}
          value={note}
          spellCheck
          onChange={(e) => onChange(e.target.value)}
        />
        {note.trim() && (
          <div className="orr-md mt-3 rounded-md bg-background/40 p-3 text-sm">
            <ReactMarkdown remarkPlugins={[remarkGfm]}>{note}</ReactMarkdown>
          </div>
        )}
      </section>
    </div>
  );
}
