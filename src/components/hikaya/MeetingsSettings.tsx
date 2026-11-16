import React, { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { save } from "@tauri-apps/plugin-dialog";
import { ArrowLeft, Circle, Trash2 } from "lucide-react";
import {
  commands,
  type ActiveMeeting,
  type ExportFormat,
  type MeetingDetail,
  type MeetingLine,
  type MeetingSummary,
} from "@/bindings";
import { notify } from "@/stores/noticeStore";
import { MarkdownContent } from "../whats-new/MarkdownContent";
import { Button } from "../ui/Button";
import { Input } from "../ui/Input";
import { SettingsGroup } from "../ui/SettingsGroup";

/** Event the backend sends when a meeting changes. */
const UPDATED_EVENT = "hikaya-meeting-updated";
/** While recording, the screen also refreshes on this interval. */
const POLL_MS = 3000;

const clock = (ms: number) => {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const mm = String(m).padStart(2, "0");
  const ss = String(s).padStart(2, "0");
  return h > 0 ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
};

type Result<T> = { status: "ok"; data: T } | { status: "error"; error: string };

const Transcript: React.FC<{ lines: MeetingLine[] }> = ({ lines }) => {
  const { t } = useTranslation();
  return (
    <ol className="flex flex-col gap-2 px-4 py-3">
      {lines.map((line, i) => (
        <li key={`${line.start_ms}-${line.speaker}-${i}`} className="text-sm">
          <span className="text-xs text-ink-3 tabular-nums me-2">
            {clock(line.start_ms)}
          </span>
          <span
            className={`font-semibold me-1 ${line.speaker === "you" ? "text-logo-primary" : ""}`}
          >
            {t(`hikaya.meetings.speakers.${line.speaker}`)}:
          </span>
          <span className="whitespace-pre-wrap">{line.text}</span>
        </li>
      ))}
    </ol>
  );
};

const Detail: React.FC<{ id: number; onBack: () => void }> = ({
  id,
  onBack,
}) => {
  const { t, i18n } = useTranslation();
  const [detail, setDetail] = useState<MeetingDetail | null>(null);
  const [title, setTitle] = useState("");
  const [summarizing, setSummarizing] = useState(false);
  const [copied, setCopied] = useState(false);

  const load = useCallback(async () => {
    const result = await commands.hikayaMeetingGet(id);
    if (result.status === "ok" && result.data) {
      setDetail(result.data);
      setTitle((current) => current || result.data!.meeting.title);
    }
  }, [id]);

  useEffect(() => {
    void load();
    const unlisten = listen<number>(UPDATED_EVENT, (event) => {
      if (event.payload === id) void load();
    });
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, [id, load]);

  const failed = (key: string, result: Result<unknown>) => {
    if (result.status === "error") {
      notify({ title: t(key), detail: result.error });
      return true;
    }
    return false;
  };

  const speakers = {
    you: t("hikaya.meetings.speakers.you"),
    others: t("hikaya.meetings.speakers.others"),
  };

  const exportText = async (format: ExportFormat) => {
    if (!detail) return null;
    const result = await commands.hikayaMeetingExport(id, format, {
      ...speakers,
      summary: t("hikaya.meetings.detail.summary"),
      transcript: t("hikaya.meetings.detail.transcript"),
      started: new Date(detail.meeting.started_at).toLocaleString(
        i18n.language,
      ),
    });
    return failed("hikaya.meetings.errors.export", result) ||
      result.status !== "ok"
      ? null
      : result.data;
  };

  const copy = async () => {
    const text = await exportText("markdown");
    if (text === null) return;
    await navigator.clipboard.writeText(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  const saveAs = async (format: ExportFormat) => {
    const text = await exportText(format);
    if (text === null || !detail) return;
    const extension = format === "markdown" ? "md" : "srt";
    const path = await save({
      defaultPath: `${detail.meeting.title.replace(/[\\/:*?"<>|]/g, "-")}.${extension}`,
      filters: [{ name: extension.toUpperCase(), extensions: [extension] }],
    });
    if (!path) return;
    failed(
      "hikaya.meetings.errors.export",
      await commands.hikayaMeetingSaveExport(path, text),
    );
  };

  const summarize = async () => {
    setSummarizing(true);
    const result = await commands.hikayaMeetingSummarize(
      id,
      speakers.you,
      speakers.others,
    );
    setSummarizing(false);
    if (!failed("hikaya.meetings.errors.summary", result)) await load();
  };

  const rename = async () => {
    if (
      !failed(
        "hikaya.meetings.errors.rename",
        await commands.hikayaMeetingRename(id, title),
      )
    )
      await load();
  };

  const remove = async () => {
    if (!window.confirm(t("hikaya.meetings.detail.deleteConfirm"))) return;
    if (
      !failed(
        "hikaya.meetings.errors.delete",
        await commands.hikayaMeetingDelete(id),
      )
    )
      onBack();
  };

  if (!detail) return null;
  const { meeting, lines, summary } = detail;
  const busy =
    meeting.status === "recording" || meeting.status === "transcribing";

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <button
        type="button"
        onClick={onBack}
        className="flex items-center gap-1 text-sm text-ink-3 hover:text-logo-primary cursor-pointer"
      >
        <ArrowLeft width={14} height={14} />
        {t("hikaya.meetings.detail.back")}
      </button>

      <form
        className="flex items-end gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          void rename();
        }}
      >
        <label className="flex flex-col gap-1 text-xs text-ink-3 grow">
          {t("hikaya.meetings.detail.rename")}
          <Input
            type="text"
            value={title}
            maxLength={120}
            onChange={(e) => setTitle(e.target.value)}
          />
        </label>
        <Button
          type="submit"
          variant="secondary"
          size="md"
          disabled={!title.trim() || title.trim() === meeting.title}
        >
          {t("hikaya.meetings.detail.save")}
        </Button>
      </form>
      <p className="text-xs text-ink-3">
        {new Date(meeting.started_at).toLocaleString(i18n.language)} ·{" "}
        {clock(meeting.duration_ms)} ·{" "}
        {t(`hikaya.meetings.status.${meeting.status}`)}
      </p>

      <SettingsGroup title={t("hikaya.meetings.detail.summary")}>
        <div className="px-4 py-3 space-y-3">
          {summary ? (
            <div className="text-sm">
              <MarkdownContent markdown={summary} />
            </div>
          ) : (
            <p className="text-sm text-ink-3">
              {t("hikaya.meetings.detail.noSummary")}
            </p>
          )}
          <Button
            variant="primary"
            size="md"
            disabled={summarizing || busy || lines.length === 0}
            onClick={summarize}
          >
            {summarizing
              ? t("hikaya.meetings.detail.summarizing")
              : summary
                ? t("hikaya.meetings.detail.resummarize")
                : t("hikaya.meetings.detail.summarize")}
          </Button>
        </div>
      </SettingsGroup>

      <SettingsGroup title={t("hikaya.meetings.detail.transcript")}>
        {lines.length ? (
          <Transcript lines={lines} />
        ) : (
          <p className="px-4 py-3 text-sm text-ink-3">
            {t("hikaya.meetings.detail.noLines")}
          </p>
        )}
      </SettingsGroup>

      <div className="flex flex-wrap gap-2">
        <Button variant="secondary" size="md" onClick={copy}>
          {copied
            ? t("hikaya.meetings.detail.copied")
            : t("hikaya.meetings.detail.copy")}
        </Button>
        <Button
          variant="secondary"
          size="md"
          onClick={() => saveAs("markdown")}
        >
          {t("hikaya.meetings.detail.saveMarkdown")}
        </Button>
        <Button variant="secondary" size="md" onClick={() => saveAs("srt")}>
          {t("hikaya.meetings.detail.saveSrt")}
        </Button>
        <Button
          variant="danger-ghost"
          size="md"
          className="ms-auto inline-flex items-center gap-1"
          disabled={busy}
          onClick={remove}
        >
          <Trash2 width={14} height={14} />
          {t("hikaya.meetings.detail.delete")}
        </Button>
      </div>
    </div>
  );
};

export const MeetingsSettings: React.FC = () => {
  const { t, i18n } = useTranslation();
  const [active, setActive] = useState<ActiveMeeting | null>(null);
  const [meetings, setMeetings] = useState<MeetingSummary[]>([]);
  const [live, setLive] = useState<MeetingLine[]>([]);
  const [title, setTitle] = useState("");
  const [open, setOpen] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [now, setNow] = useState(Date.now());

  const refresh = useCallback(async () => {
    const current = await commands.hikayaMeetingActive();
    setActive(current);
    const list = await commands.hikayaMeetingsList();
    if (list.status === "ok") setMeetings(list.data);
    if (current) {
      const detail = await commands.hikayaMeetingGet(current.id);
      if (detail.status === "ok" && detail.data) setLive(detail.data.lines);
    } else {
      setLive([]);
    }
  }, []);

  useEffect(() => {
    void refresh();
    const unlisten = listen<number>(UPDATED_EVENT, () => void refresh());
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, [refresh]);

  // The elapsed time and the transcript keep moving while recording.
  useEffect(() => {
    if (!active) return;
    const tick = setInterval(() => setNow(Date.now()), 1000);
    const poll = setInterval(() => void refresh(), POLL_MS);
    return () => {
      clearInterval(tick);
      clearInterval(poll);
    };
  }, [active, refresh]);

  const run = async (key: string, action: () => Promise<Result<unknown>>) => {
    setBusy(true);
    try {
      const result = await action();
      if (result.status === "error") {
        notify({ title: t(key), detail: result.error });
      }
    } finally {
      await refresh();
      setBusy(false);
    }
  };

  if (open !== null) {
    return (
      <Detail
        id={open}
        onBack={() => {
          setOpen(null);
          void refresh();
        }}
      />
    );
  }

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      {active ? (
        <SettingsGroup>
          <div className="flex flex-col gap-2 px-4 py-3">
            <div className="flex items-center justify-between gap-3">
              <span
                role="status"
                aria-label={t("hikaya.meetings.recording.label")}
                className="flex items-center gap-2 text-sm font-semibold"
              >
                <Circle
                  width={12}
                  height={12}
                  className="text-red-500 fill-red-500 animate-pulse"
                  aria-hidden="true"
                />
                {t("hikaya.meetings.recording.label")} ·{" "}
                <span className="tabular-nums">
                  {clock(now - active.started_at)}
                </span>
              </span>
              <Button
                variant="danger"
                size="md"
                disabled={busy}
                onClick={() =>
                  run("hikaya.meetings.errors.stop", async () =>
                    commands.hikayaMeetingStop(),
                  )
                }
              >
                {t("hikaya.meetings.recording.stop")}
              </Button>
            </div>
            <p className="text-sm font-medium">{active.title}</p>
            <p className="text-xs text-ink-3">
              {t("hikaya.meetings.recording.microphone", {
                name: active.microphone,
              })}
              {active.system_audio && (
                <>
                  {" · "}
                  {t("hikaya.meetings.recording.system", {
                    name: active.system_audio,
                  })}
                </>
              )}
            </p>
            {!active.system_audio && (
              <p className="text-sm text-amber-600">
                {t("hikaya.meetings.recording.noSystem")}
              </p>
            )}
          </div>
          {live.length ? (
            <Transcript lines={live} />
          ) : (
            <p className="px-4 py-3 text-sm text-ink-3">
              {t("hikaya.meetings.recording.liveEmpty")}
            </p>
          )}
        </SettingsGroup>
      ) : (
        <SettingsGroup
          title={t("hikaya.meetings.start.title")}
          description={t("hikaya.meetings.start.description")}
        >
          <form
            className="flex flex-col gap-3 px-4 py-3"
            onSubmit={(e) => {
              e.preventDefault();
              void run("hikaya.meetings.errors.start", async () => {
                const result = await commands.hikayaMeetingStart(title);
                if (result.status === "ok") setTitle("");
                return result;
              });
            }}
          >
            <div className="flex items-center gap-2">
              <Input
                type="text"
                className="grow"
                value={title}
                maxLength={120}
                aria-label={t("hikaya.meetings.start.placeholder")}
                placeholder={t("hikaya.meetings.start.placeholder")}
                onChange={(e) => setTitle(e.target.value)}
              />
              <Button type="submit" variant="primary" size="md" disabled={busy}>
                {t("hikaya.meetings.start.button")}
              </Button>
            </div>
            <p className="text-xs text-ink-3">
              {t("hikaya.meetings.start.consent")}
            </p>
          </form>
        </SettingsGroup>
      )}

      <SettingsGroup title={t("hikaya.meetings.list.title")}>
        {meetings.filter((m) => m.id !== active?.id).length === 0 ? (
          <p className="px-4 py-3 text-sm text-ink-3">
            {t("hikaya.meetings.list.empty")}
          </p>
        ) : (
          meetings
            .filter((m) => m.id !== active?.id)
            .map((m) => (
              <button
                key={m.id}
                type="button"
                onClick={() => setOpen(m.id)}
                className="w-full flex items-center justify-between gap-3 px-4 py-2 text-start hover:bg-logo-primary/10 cursor-pointer"
              >
                <span className="flex flex-col min-w-0">
                  <span className="text-sm font-medium truncate">
                    {m.title}
                  </span>
                  <span className="text-xs text-ink-3">
                    {new Date(m.started_at).toLocaleString(i18n.language)} ·{" "}
                    {clock(m.duration_ms)} ·{" "}
                    {t("hikaya.meetings.list.lines", { count: m.line_count })}
                  </span>
                </span>
                <span className="text-xs text-ink-3 shrink-0">
                  {t(`hikaya.meetings.status.${m.status}`)}
                </span>
              </button>
            ))
        )}
      </SettingsGroup>
    </div>
  );
};
