import type { Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const defaultSettings: Record<string, unknown> = JSON.parse(
  readFileSync(
    fileURLToPath(new URL("./default-settings.json", import.meta.url)),
    "utf8",
  ),
);

/** What a test wants the fake backend to answer for a command. */
export type CommandHandlers = Record<string, (args: any) => unknown>;

export interface MockProposal {
  pattern: string;
  replacement: string;
  count: number;
}

export interface MockOptions {
  /** Overrides for the app settings. */
  settings?: Record<string, unknown>;
  /** Rule suggestions the fake backend starts with. */
  proposals?: MockProposal[];
  /** Version the fake app reports. */
  appVersion?: string;
  /** Meetings the fake backend starts with. */
  meetings?: Record<string, unknown>[];
  /** Starting state of the fake hub connection. */
  hub?: Record<string, unknown>;
  /** Dictations the fake History starts with. */
  history?: { id: number; text: string }[];
  /** UI language, as the OS would report it. */
  locale?: string;
}

/**
 * Runs the real frontend in a plain browser by standing in for Tauri.
 *
 * Commands answer from `window.__HIKAYA_MOCK__.handlers` (set per test with
 * `page.evaluate`, or through the built-in learning backend below). Unknown
 * commands answer `null` and are listed in `window.__HIKAYA_MOCK__.unknown`.
 */
export const mockTauri = async (page: Page, options: MockOptions = {}) => {
  const settings = { ...defaultSettings, ...options.settings };
  await page.addInitScript(
    ({ settings, locale, proposals, history, hub, appVersion, meetings }) => {
      type Rule = {
        id: number;
        pattern: string;
        replacement: string;
        enabled: boolean;
        source: string;
        created_at: number;
      };
      const state = {
        rules: [] as Rule[],
        proposals: [...proposals],
        nextId: 1,
        calls: [] as { cmd: string; args: unknown }[],
        unknown: [] as string[],
        hub: {
          state: "not_connected",
          url: null as string | null,
          device_name: null as string | null,
          message: null as string | null,
          pending: 0,
          last_sync_at: null as number | null,
        },
        hubPassphrase: "correct horse battery staple",
        meetings: [...meetings] as any[],
        activeMeeting: null as any,
        saved: [] as { path: string; contents: string }[],
        history: history.map(({ id, text }) => ({
          id,
          file_name: `hikaya-${id}.wav`,
          timestamp: 1763200000 + id,
          saved: false,
          title: "",
          transcription_text: text,
          post_processed_text: null,
          post_process_prompt: null,
          post_process_requested: false,
          source_name: null,
          language: "fr",
          language_source: null,
        })) as Record<string, unknown>[],
      };
      const callbacks = new Map<number, (payload: unknown) => void>();
      let nextCallback = 1;

      const learning: Record<string, (args: any) => unknown> = {
        hikaya_list_rules: () => [...state.rules].reverse(),
        hikaya_list_proposals: () => state.proposals,
        hikaya_add_rule: ({ pattern, replacement }) => {
          const rule = {
            id: state.nextId++,
            pattern: pattern.trim(),
            replacement: replacement.trim(),
            enabled: true,
            source: "manual",
            created_at: 0,
          };
          state.rules.push(rule);
          return rule;
        },
        hikaya_set_rule_enabled: ({ id, enabled }) => {
          const rule = state.rules.find((r) => r.id === id);
          if (rule) rule.enabled = enabled;
          return null;
        },
        hikaya_delete_rule: ({ id }) => {
          state.rules = state.rules.filter((r) => r.id !== id);
          return null;
        },
        hikaya_accept_proposal: ({ pattern, replacement }) => {
          state.proposals = state.proposals.filter(
            (p) => p.pattern !== pattern,
          );
          const rule = {
            id: state.nextId++,
            pattern,
            replacement,
            enabled: true,
            source: "learned",
            created_at: 0,
          };
          state.rules.push(rule);
          return rule;
        },
        hikaya_dismiss_proposal: ({ pattern }) => {
          state.proposals = state.proposals.filter(
            (p) => p.pattern !== pattern,
          );
          return null;
        },
        hikaya_meeting_active: () => state.activeMeeting,
        hikaya_meetings_list: () =>
          state.meetings.map((m) => ({
            id: m.id,
            title: m.title,
            started_at: m.started_at,
            duration_ms: m.duration_ms,
            status: m.status,
            system_audio: m.system_audio,
            has_summary: !!m.summary,
            line_count: m.lines.length,
          })),
        hikaya_meeting_get: ({ id }) => {
          const m = state.meetings.find((x) => x.id === id);
          if (!m) return null;
          return {
            meeting: {
              id: m.id,
              title: m.title,
              started_at: m.started_at,
              duration_ms: m.duration_ms,
              status: m.status,
              system_audio: m.system_audio,
              has_summary: !!m.summary,
              line_count: m.lines.length,
            },
            lines: m.lines,
            summary: m.summary ?? null,
          };
        },
        hikaya_meeting_start: ({ title }) => {
          const id = state.meetings.length + 100;
          state.activeMeeting = {
            id,
            title: title.trim() || "2026-11-16 18:00",
            started_at: Date.now() - 65_000,
            microphone: "Laptop microphone",
            system_audio: "Monitor of Speakers",
            system_audio_error: null,
          };
          state.meetings.push({
            id,
            title: state.activeMeeting.title,
            started_at: state.activeMeeting.started_at,
            duration_ms: 0,
            status: "recording",
            system_audio: true,
            lines: [
              {
                start_ms: 0,
                end_ms: 30000,
                speaker: "others",
                text: "Bonjour, on commence ?",
              },
            ],
          });
          return state.activeMeeting;
        },
        hikaya_meeting_stop: () => {
          const m = state.meetings.find(
            (x) => x.id === state.activeMeeting?.id,
          );
          if (m) Object.assign(m, { status: "done", duration_ms: 65_000 });
          const id = state.activeMeeting?.id;
          state.activeMeeting = null;
          return id;
        },
        hikaya_meeting_summarize: ({ id }) => {
          const m = state.meetings.find((x) => x.id === id);
          m.summary =
            "## TL;DR\nLa démo est décalée à jeudi.\n\n## Action items\n- [ ] You: prévenir Sarah";
          return m.summary;
        },
        hikaya_meeting_rename: ({ id, title }) => {
          state.meetings.find((x) => x.id === id).title = title.trim();
          return null;
        },
        hikaya_meeting_delete: ({ id }) => {
          state.meetings = state.meetings.filter((x) => x.id !== id);
          return null;
        },
        hikaya_meeting_export: ({ id, format, labels }) => {
          const m = state.meetings.find((x) => x.id === id);
          return format === "srt"
            ? "1\n00:00:00,000 --> 00:00:30,000\n"
            : `# ${m.title}\n\n## ${labels.transcript}\n`;
        },
        hikaya_meeting_save_export: ({ path, contents }) => {
          state.saved.push({ path, contents });
          return null;
        },
        "plugin:dialog|save": () => "/home/user/Documents/meeting.md",
        hikaya_hub_status: () => ({ ...state.hub }),
        hikaya_hub_connect: ({ url, deviceName, passphrase }) => {
          if (!/^https?:\/\//.test(url)) {
            throw "the address must start with http:// or https://";
          }
          if (passphrase !== state.hubPassphrase)
            throw "the passphrase is wrong";
          Object.assign(state.hub, {
            state: "up_to_date",
            url: url.replace(/\/+$/, ""),
            device_name: deviceName.trim(),
            pending: 0,
            last_sync_at: 1763300000000,
          });
          return { ...state.hub };
        },
        hikaya_hub_unlock: ({ passphrase }) => {
          if (passphrase !== state.hubPassphrase)
            throw "the passphrase is wrong";
          Object.assign(state.hub, { state: "up_to_date", pending: 0 });
          return { ...state.hub };
        },
        hikaya_hub_sync_now: () => {
          Object.assign(state.hub, { state: "up_to_date", pending: 0 });
          return { ...state.hub };
        },
        hikaya_hub_disconnect: () => {
          Object.assign(state.hub, {
            state: "not_connected",
            url: null,
            device_name: null,
            pending: 0,
            last_sync_at: null,
          });
          return { ...state.hub };
        },
        hikaya_correct_history_entry: ({ id, correctedText }) => {
          const entry = state.history.find((e) => e.id === id);
          if (entry) entry.transcription_text = correctedText;
          return [{ from: "a med", to: "Ahmed" }];
        },
      };

      const core: Record<string, (args: any) => unknown> = {
        get_app_settings: () => settings,
        get_default_settings: () => settings,
        get_history_entries: () => ({
          entries: state.history,
          has_more: false,
          next_cursor: null,
        }),
        get_available_models: () => [],
        get_retired_models: () => [],
        get_active_notices: () => [],
        get_current_model: () => "",
        check_custom_sounds: () => ({ start: false, stop: false }),
        is_update_checks_locked: () => false,
        get_available_microphones: () => [],
        get_available_output_devices: () => [],
        get_microphone_channels: () => [],
        "plugin:event|listen": () => nextCallback++,
        "plugin:event|unlisten": () => null,
        "plugin:app|version": () => appVersion,
      };

      Object.assign(state.hub, hub);
      (window as any).__HIKAYA_MOCK__ = { state, handlers: {} };
      (window as any).__TAURI_OS_PLUGIN_INTERNALS__ = {
        os_type: "linux",
        platform: "linux",
        family: "unix",
        arch: "x86_64",
        version: "6.0",
        eol: "\n",
        exe_extension: "",
      };
      (window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
        unregisterListener: () => {},
      };
      (window as any).__TAURI_INTERNALS__ = {
        metadata: {
          currentWindow: { label: "main" },
          currentWebview: { label: "main", windowLabel: "main" },
        },
        transformCallback: (callback: (payload: unknown) => void) => {
          const id = nextCallback++;
          callbacks.set(id, callback);
          return id;
        },
        unregisterCallback: (id: number) => callbacks.delete(id),
        convertFileSrc: (path: string) => path,
        invoke: async (cmd: string, args: unknown) => {
          state.calls.push({ cmd, args });
          const custom = (window as any).__HIKAYA_MOCK__.handlers[cmd];
          const handler = custom ?? learning[cmd] ?? core[cmd];
          if (handler) return handler(args);
          if (cmd === "plugin:os|locale") return locale;
          if (!state.unknown.includes(cmd)) state.unknown.push(cmd);
          return null;
        },
      };
    },
    {
      settings,
      locale: options.locale ?? "en-US",
      proposals: options.proposals ?? [],
      history: options.history ?? [],
      hub: options.hub ?? {},
      appVersion: options.appVersion ?? "0.2.0",
      meetings: options.meetings ?? [],
    },
  );
};
