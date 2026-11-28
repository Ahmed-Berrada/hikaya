/**
 * Shows background failures Hikaya reports from Rust (`hikaya/problems.rs`):
 * a dictation that could not be queued for the hub, a meeting with parts
 * missing, a phone recording that could not be transcribed. They stay in the
 * footer until dismissed, like a file Handy can't read.
 */
import { listen } from "@tauri-apps/api/event";
import i18n from "@/i18n";
import { notify } from "@/stores/noticeStore";

export type ProblemKind =
  | "sync_queue"
  | "meeting_incomplete"
  | "phone_recording"
  | "command_no_selection"
  | "command_no_model"
  | "command_failed"
  | "command_translated"
  | "command_unsupported"
  | "command_focus_moved";

export interface Problem {
  kind: ProblemKind;
  count: number | null;
  detail: string | null;
}

/** The notice for a problem, in the current language. */
export const problemNotice = (problem: Problem) => ({
  key: `hikaya-${problem.kind}`,
  level: "error" as const,
  title: i18n.t(`hikaya.problems.${problem.kind}.title`, {
    count: problem.count ?? 1,
  }),
  detail: [
    i18n.t(`hikaya.problems.${problem.kind}.detail`, {
      count: problem.count ?? 1,
    }),
    problem.detail,
  ]
    .filter(Boolean)
    .join(" "),
});

let listening = false;

/** Starts showing problems. Safe to call more than once. */
export const listenForProblems = () => {
  if (listening) return;
  listening = true;
  void listen<Problem>("hikaya-problem", (event) =>
    notify(problemNotice(event.payload)),
  );
};
