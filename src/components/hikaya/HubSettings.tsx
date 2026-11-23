import React, { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type SyncState, type SyncStatus } from "@/bindings";
import { notify } from "@/stores/noticeStore";
import { Button } from "../ui/Button";
import { Input } from "../ui/Input";
import { SettingsGroup } from "../ui/SettingsGroup";

/** How often the screen refreshes the status while it is open. */
const REFRESH_MS = 5000;

const STATE_TONE: Record<SyncState, string> = {
  not_connected: "bg-line",
  waiting: "bg-line",
  syncing: "bg-logo-primary",
  up_to_date: "bg-green-500",
  offline: "bg-amber-500",
  locked: "bg-amber-500",
  unauthorized: "bg-red-500",
  error: "bg-red-500",
};

const NEEDS_EXPLANATION: SyncState[] = [
  "offline",
  "locked",
  "unauthorized",
  "error",
];

const Field: React.FC<{ label: string; children: React.ReactNode }> = ({
  label,
  children,
}) => (
  <label className="flex flex-col gap-1 text-xs text-ink-3">
    {label}
    {children}
  </label>
);

export const HubSettings: React.FC = () => {
  const { t, i18n } = useTranslation();
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [url, setUrl] = useState("");
  const [deviceName, setDeviceName] = useState("");
  const [passphrase, setPassphrase] = useState("");
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    setStatus(await commands.hikayaHubStatus());
  }, []);

  useEffect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), REFRESH_MS);
    return () => clearInterval(timer);
  }, [refresh]);

  /** Runs one hub action; on failure shows why and keeps the form as typed. */
  const act = async (
    action: () => Promise<
      { status: "ok"; data: SyncStatus } | { status: "error"; error: string }
    >,
  ): Promise<boolean> => {
    setBusy(true);
    try {
      const result = await action();
      if (result.status === "error") {
        notify({ title: t("hikaya.hub.failed"), detail: result.error });
        return false;
      }
      setStatus(result.data);
      return true;
    } finally {
      setBusy(false);
    }
  };

  const connect = async () => {
    const ok = await act(() =>
      commands.hikayaHubConnect(url, deviceName, passphrase),
    );
    // The passphrase is never kept, not even in this form.
    setPassphrase("");
    if (ok) setUrl("");
  };

  const unlock = async () => {
    await act(() => commands.hikayaHubUnlock(passphrase));
    setPassphrase("");
  };

  const disconnect = async () => {
    if (!window.confirm(t("hikaya.hub.disconnectConfirm"))) return;
    await act(async () => commands.hikayaHubDisconnect());
  };

  if (!status) return null;
  const connected = status.state !== "not_connected";
  const lastSync = status.last_sync_at
    ? t("hikaya.hub.lastSync", {
        when: new Date(status.last_sync_at).toLocaleString(i18n.language),
      })
    : t("hikaya.hub.never");

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <SettingsGroup
        title={t("hikaya.hub.title")}
        description={t("hikaya.hub.description")}
      >
        <div className="flex flex-col gap-1 px-4 py-3">
          <div className="flex items-center gap-2">
            <span
              aria-hidden="true"
              className={`inline-block w-2.5 h-2.5 rounded-full ${STATE_TONE[status.state]}`}
            />
            <span
              role="status"
              aria-label={t("hikaya.hub.statusLabel")}
              className="text-sm font-semibold"
            >
              {t(`hikaya.hub.states.${status.state}`)}
            </span>
          </div>
          {connected && (
            <p className="text-xs text-ink-3">
              {t("hikaya.hub.connectedTo", {
                url: status.url,
                name: status.device_name,
              })}
            </p>
          )}
          {NEEDS_EXPLANATION.includes(status.state) && (
            <p className="text-sm">
              {t(`hikaya.hub.explain.${status.state}`)}
              {status.message && status.state === "error" && (
                <span className="block text-xs text-ink-3">
                  {status.message}
                </span>
              )}
            </p>
          )}
          {connected && (
            <p className="text-xs text-ink-3">
              {status.pending > 0
                ? t("hikaya.hub.pending", { count: status.pending })
                : t("hikaya.hub.nothingPending")}
              {" · "}
              {lastSync}
              {status.refused > 0 && (
                <span className="block text-warning">
                  {t("hikaya.hub.refused", { count: status.refused })}
                </span>
              )}
            </p>
          )}
        </div>

        {!connected && (
          <form
            className="flex flex-col gap-3 px-4 py-3"
            onSubmit={(e) => {
              e.preventDefault();
              void connect();
            }}
          >
            <Field label={t("hikaya.hub.address")}>
              <Input
                type="url"
                value={url}
                required
                placeholder="https://hub.example.ts.net"
                onChange={(e) => setUrl(e.target.value)}
              />
            </Field>
            <Field label={t("hikaya.hub.deviceName")}>
              <Input
                type="text"
                value={deviceName}
                required
                maxLength={64}
                placeholder={t("hikaya.hub.deviceNamePlaceholder")}
                onChange={(e) => setDeviceName(e.target.value)}
              />
            </Field>
            <Field label={t("hikaya.hub.passphrase")}>
              <Input
                type="password"
                value={passphrase}
                required
                autoComplete="current-password"
                onChange={(e) => setPassphrase(e.target.value)}
              />
            </Field>
            <p className="text-xs text-ink-3">
              {t("hikaya.hub.passphraseHint")}
            </p>
            <div className="flex justify-end">
              <Button
                type="submit"
                variant="primary"
                size="md"
                disabled={busy || !url || !deviceName || !passphrase}
              >
                {busy ? t("hikaya.hub.connecting") : t("hikaya.hub.connect")}
              </Button>
            </div>
          </form>
        )}

        {status.state === "locked" && (
          <form
            className="flex items-end gap-2 px-4 py-3"
            onSubmit={(e) => {
              e.preventDefault();
              void unlock();
            }}
          >
            <Field label={t("hikaya.hub.passphrase")}>
              <Input
                type="password"
                className="w-64"
                value={passphrase}
                autoComplete="current-password"
                onChange={(e) => setPassphrase(e.target.value)}
              />
            </Field>
            <Button
              type="submit"
              variant="primary"
              size="md"
              disabled={busy || !passphrase}
            >
              {t("hikaya.hub.unlock")}
            </Button>
          </form>
        )}

        {connected && (
          <div className="flex justify-end gap-2 px-4 py-3">
            <Button
              variant="secondary"
              size="md"
              disabled={busy}
              onClick={disconnect}
            >
              {t("hikaya.hub.disconnect")}
            </Button>
            <Button
              variant="primary"
              size="md"
              disabled={busy || status.state === "syncing"}
              onClick={() => act(() => commands.hikayaHubSyncNow())}
            >
              {t("hikaya.hub.syncNow")}
            </Button>
          </div>
        )}
      </SettingsGroup>
    </div>
  );
};
