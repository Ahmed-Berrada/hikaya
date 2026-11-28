import React, { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ArrowRight, Trash2 } from "lucide-react";
import {
  commands,
  type Prefs,
  type RuleProposal,
  type Stats,
  type StoredRule,
  type StoredSnippet,
} from "@/bindings";
import { notify } from "@/stores/noticeStore";
import { isComposingKeyEvent } from "@/lib/utils/keyboard";
import Badge from "../ui/Badge";
import { Button } from "../ui/Button";
import { Input } from "../ui/Input";
import { Textarea } from "../ui/Textarea";
import { SettingsGroup } from "../ui/SettingsGroup";
import { Switch } from "../ui/Switch";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { ShortcutInput } from "../settings/ShortcutInput";

const Replacement: React.FC<{ from: string; to: string }> = ({ from, to }) => (
  <span className="flex items-center gap-2 min-w-0 text-sm">
    <span className="truncate text-ink-3 line-through">{from}</span>
    <ArrowRight width={14} height={14} className="shrink-0 text-ink-3" />
    <span className="truncate font-semibold">{to}</span>
  </span>
);

const Figure: React.FC<{ label: string; value: string; hint?: string }> = ({
  label,
  value,
  hint,
}) => (
  <div className="flex flex-col gap-0.5" title={hint}>
    <span className="text-xs text-ink-3">{label}</span>
    <span className="text-lg font-semibold tabular-nums">{value}</span>
  </div>
);

const StatsPanel: React.FC<{ stats: Stats }> = ({ stats }) => {
  const { t, i18n } = useTranslation();
  const n = (value: number) => value.toLocaleString(i18n.language);
  return (
    <SettingsGroup
      title={t("hikaya.learning.stats.title")}
      description={t("hikaya.learning.stats.description")}
    >
      <div className="grid grid-cols-2 sm:grid-cols-4 gap-4 px-4 py-3">
        <Figure
          label={t("hikaya.learning.stats.today")}
          value={n(stats.words_today)}
        />
        <Figure
          label={t("hikaya.learning.stats.week")}
          value={n(stats.words_week)}
        />
        <Figure
          label={t("hikaya.learning.stats.total")}
          value={n(stats.words_total)}
        />
        <Figure
          label={t("hikaya.learning.stats.saved")}
          value={t("hikaya.learning.stats.savedValue", {
            count: stats.minutes_saved,
          })}
          hint={t("hikaya.learning.stats.savedHint")}
        />
      </div>
      <p className="px-4 py-2 text-xs text-ink-3">
        {t("hikaya.learning.stats.dictations", {
          count: stats.dictations_total,
        })}
        {" · "}
        {t("hikaya.learning.stats.rules", {
          count: stats.rules,
          learned: stats.learned_rules,
        })}
      </p>
      {stats.top_corrections.length > 0 && (
        <div className="px-4 py-3">
          <p className="text-xs text-ink-3 mb-2">
            {t("hikaya.learning.stats.topCorrections")}
          </p>
          <ul className="flex flex-col gap-1">
            {stats.top_corrections.map((c) => (
              <li
                key={`${c.from}\u0000${c.to}`}
                className="flex items-center justify-between gap-3"
              >
                <Replacement from={c.from} to={c.to} />
                <span className="text-xs text-ink-3 tabular-nums">
                  {t("hikaya.learning.stats.times", { count: c.count })}
                </span>
              </li>
            ))}
          </ul>
        </div>
      )}
    </SettingsGroup>
  );
};

/** What each spoken command does, and the phrases that say it. */
const VOICE_COMMANDS = ["newLine", "newParagraph", "scratch"] as const;

/** What changes in each kind of app when dictation adapts to it. */
const APP_KINDS = ["chat", "terminal", "code"] as const;

/** Hikaya's own switches: voice commands, and adapting to the app. */
const PrefsPanels: React.FC = () => {
  const { t } = useTranslation();
  const [prefs, setPrefs] = useState<Prefs | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    void commands.hikayaGetPrefs().then(setPrefs);
  }, []);

  const save = async (change: Partial<Prefs>) => {
    if (!prefs) return;
    const next = { ...prefs, ...change };
    setSaving(true);
    setPrefs(next);
    const result = await commands.hikayaSetPrefs(next);
    if (result.status === "error") {
      setPrefs(prefs);
      notify({
        title: t("hikaya.learning.voiceCommands.saveError"),
        detail: result.error,
      });
    }
    setSaving(false);
  };

  if (!prefs) return null;
  return (
    <>
      <SettingsGroup
        title={t("hikaya.learning.voiceCommands.title")}
        description={t("hikaya.learning.voiceCommands.description")}
      >
        <ToggleSwitch
          checked={prefs.voice_commands}
          onChange={(on) => void save({ voice_commands: on })}
          isUpdating={saving}
          label={t("hikaya.learning.voiceCommands.toggle")}
          description={t("hikaya.learning.voiceCommands.toggleHint")}
          grouped
        />
        <ul className="px-4 py-3 space-y-1.5 text-sm">
          {VOICE_COMMANDS.map((key) => (
            <li key={key} className="flex flex-wrap justify-between gap-x-4">
              <span className="font-semibold">
                {t(`hikaya.learning.voiceCommands.${key}`)}
              </span>
              <span className="text-ink-3">
                {t(`hikaya.learning.voiceCommands.${key}Says`)}
              </span>
            </li>
          ))}
        </ul>
      </SettingsGroup>
      <SettingsGroup
        title={t("hikaya.command.title")}
        description={t("hikaya.command.description")}
      >
        <ShortcutInput shortcutId="hikaya_command" grouped />
        <p className="px-4 py-3 text-sm text-ink-3">
          {t("hikaya.command.examples")}
        </p>
      </SettingsGroup>
      <SettingsGroup
        title={t("hikaya.learning.adapt.title")}
        description={t("hikaya.learning.adapt.description")}
      >
        <ToggleSwitch
          checked={prefs.adapt_to_app}
          onChange={(on) => void save({ adapt_to_app: on })}
          isUpdating={saving}
          label={t("hikaya.learning.adapt.toggle")}
          description={t("hikaya.learning.adapt.toggleHint")}
          grouped
        />
        <ul className="px-4 py-3 space-y-1.5 text-sm">
          {APP_KINDS.map((key) => (
            <li key={key} className="flex flex-wrap justify-between gap-x-4">
              <span className="font-semibold">
                {t(`hikaya.learning.adapt.${key}`)}
              </span>
              <span className="text-ink-3">
                {t(`hikaya.learning.adapt.${key}Does`)}
              </span>
            </li>
          ))}
        </ul>
      </SettingsGroup>
    </>
  );
};

/** Saved texts typed by a spoken cue ("my address" → the address). */
const SnippetsPanel: React.FC = () => {
  const { t } = useTranslation();
  const [snippets, setSnippets] = useState<StoredSnippet[]>([]);
  const [cue, setCue] = useState("");
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);

  const reload = useCallback(async () => {
    const loaded = await commands.hikayaListSnippets();
    if (loaded.status === "ok") setSnippets(loaded.data);
    else
      notify({ title: t("hikaya.snippets.loadError"), detail: loaded.error });
  }, [t]);

  useEffect(() => {
    void reload();
  }, [reload]);

  const canSave = cue.trim().length > 0 && text.trim().length > 0 && !busy;

  const save = async () => {
    if (!canSave) return;
    setBusy(true);
    const result = await commands.hikayaSaveSnippet(cue, text);
    if (result.status === "error") {
      notify({ title: t("hikaya.snippets.saveError"), detail: result.error });
    } else {
      setCue("");
      setText("");
    }
    await reload();
    setBusy(false);
  };

  const remove = async (id: string) => {
    setBusy(true);
    const result = await commands.hikayaDeleteSnippet(id);
    if (result.status === "error") {
      notify({ title: t("hikaya.snippets.saveError"), detail: result.error });
    }
    await reload();
    setBusy(false);
  };

  return (
    <SettingsGroup
      title={t("hikaya.snippets.title")}
      description={t("hikaya.snippets.description")}
    >
      <div className="flex flex-col gap-2 px-4 py-3">
        <label className="flex flex-col gap-1 text-xs text-ink-3">
          {t("hikaya.snippets.cueLabel")}
          <Input
            type="text"
            className="w-64"
            variant="compact"
            value={cue}
            maxLength={100}
            onChange={(e) => setCue(e.target.value)}
            placeholder={t("hikaya.snippets.cuePlaceholder")}
          />
        </label>
        <label className="flex flex-col gap-1 text-xs text-ink-3">
          {t("hikaya.snippets.textLabel")}
          <Textarea
            variant="compact"
            value={text}
            maxLength={4000}
            onChange={(e) => setText(e.target.value)}
            placeholder={t("hikaya.snippets.textPlaceholder")}
          />
        </label>
        <div>
          <Button
            variant="primary"
            size="md"
            disabled={!canSave}
            onClick={() => void save()}
          >
            {t("hikaya.snippets.save")}
          </Button>
        </div>
      </div>
      {snippets.length === 0 ? (
        <p className="px-4 py-3 text-sm text-ink-3">
          {t("hikaya.snippets.empty")}
        </p>
      ) : (
        snippets.map((snippet) => (
          <div
            key={snippet.id}
            className="flex items-start justify-between gap-3 px-4 py-2"
          >
            <div className="min-w-0 text-sm">
              <div className="font-semibold">“{snippet.cue}”</div>
              <div className="whitespace-pre-line text-ink-3 line-clamp-3">
                {snippet.text}
              </div>
            </div>
            <Button
              variant="ghost"
              size="sm"
              disabled={busy}
              onClick={() => void remove(snippet.id)}
              aria-label={t("hikaya.snippets.delete", { cue: snippet.cue })}
            >
              <Trash2 width={16} height={16} />
            </Button>
          </div>
        ))
      )}
    </SettingsGroup>
  );
};

export const LearningSettings: React.FC = () => {
  const { t } = useTranslation();
  const [rules, setRules] = useState<StoredRule[]>([]);
  const [proposals, setProposals] = useState<RuleProposal[]>([]);
  const [pattern, setPattern] = useState("");
  const [replacement, setReplacement] = useState("");
  const [busy, setBusy] = useState(false);
  const [stats, setStats] = useState<Stats | null>(null);

  const reload = useCallback(async () => {
    const [loadedRules, loadedProposals] = await Promise.all([
      commands.hikayaListRules(),
      commands.hikayaListProposals(),
    ]);
    if (loadedRules.status === "ok") setRules(loadedRules.data);
    if (loadedProposals.status === "ok") setProposals(loadedProposals.data);
    const loadedStats = await commands.hikayaStats();
    if (loadedStats.status === "ok") setStats(loadedStats.data);
    const failed =
      loadedRules.status === "error"
        ? loadedRules.error
        : loadedProposals.status === "error"
          ? loadedProposals.error
          : null;
    if (failed) {
      notify({ title: t("hikaya.learning.errors.load"), detail: failed });
    }
  }, [t]);

  useEffect(() => {
    void reload();
  }, [reload]);

  /** Runs one change, reports a failure, then reloads both lists. */
  const change = async (
    action: () => Promise<
      { status: "ok" } | { status: "error"; error: string }
    >,
    errorKey: string,
  ): Promise<boolean> => {
    setBusy(true);
    try {
      const result = await action();
      if (result.status === "error") {
        notify({ title: t(errorKey), detail: result.error });
        return false;
      }
      return true;
    } finally {
      await reload();
      setBusy(false);
    }
  };

  const canAdd = pattern.trim().length > 0 && replacement.trim().length > 0;

  const addRule = async () => {
    if (!canAdd || busy) return;
    const added = await change(
      () => commands.hikayaAddRule(pattern, replacement),
      "hikaya.learning.errors.save",
    );
    if (added) {
      setPattern("");
      setReplacement("");
    }
  };

  const onEnter = (e: React.KeyboardEvent) => {
    if (isComposingKeyEvent(e.nativeEvent)) return;
    if (e.key === "Enter") {
      e.preventDefault();
      void addRule();
    }
  };

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      {stats && <StatsPanel stats={stats} />}
      <PrefsPanels />
      <SnippetsPanel />
      <SettingsGroup
        title={t("hikaya.learning.proposals.title")}
        description={t("hikaya.learning.proposals.description")}
      >
        {proposals.length === 0 ? (
          <p className="px-4 py-3 text-sm text-ink-3">
            {t("hikaya.learning.proposals.empty")}
          </p>
        ) : (
          proposals.map((proposal) => (
            <div
              key={`${proposal.pattern}\u0000${proposal.replacement}`}
              className="flex items-center justify-between gap-3 px-4 py-2"
            >
              <div className="flex flex-col min-w-0">
                <Replacement
                  from={proposal.pattern}
                  to={proposal.replacement}
                />
                <span className="text-xs text-ink-3">
                  {t("hikaya.learning.proposals.count", {
                    count: proposal.count,
                  })}
                </span>
              </div>
              <div className="flex items-center gap-2 shrink-0">
                <Button
                  variant="secondary"
                  size="sm"
                  disabled={busy}
                  onClick={() =>
                    change(
                      () =>
                        commands.hikayaDismissProposal(
                          proposal.pattern,
                          proposal.replacement,
                        ),
                      "hikaya.learning.errors.proposal",
                    )
                  }
                >
                  {t("hikaya.learning.proposals.dismiss")}
                </Button>
                <Button
                  variant="primary"
                  size="sm"
                  disabled={busy}
                  onClick={() =>
                    change(
                      () =>
                        commands.hikayaAcceptProposal(
                          proposal.pattern,
                          proposal.replacement,
                        ),
                      "hikaya.learning.errors.proposal",
                    )
                  }
                >
                  {t("hikaya.learning.proposals.accept")}
                </Button>
              </div>
            </div>
          ))
        )}
      </SettingsGroup>

      <SettingsGroup
        title={t("hikaya.learning.rules.title")}
        description={t("hikaya.learning.rules.description")}
      >
        <div className="flex flex-wrap items-end gap-2 px-4 py-3">
          <label className="flex flex-col gap-1 text-xs text-ink-3">
            {t("hikaya.learning.rules.patternLabel")}
            <Input
              type="text"
              className="w-44"
              variant="compact"
              value={pattern}
              maxLength={80}
              onChange={(e) => setPattern(e.target.value)}
              onKeyDown={onEnter}
              placeholder={t("hikaya.learning.rules.patternPlaceholder")}
            />
          </label>
          <label className="flex flex-col gap-1 text-xs text-ink-3">
            {t("hikaya.learning.rules.replacementLabel")}
            <Input
              type="text"
              className="w-44"
              variant="compact"
              value={replacement}
              maxLength={80}
              onChange={(e) => setReplacement(e.target.value)}
              onKeyDown={onEnter}
              placeholder={t("hikaya.learning.rules.replacementPlaceholder")}
            />
          </label>
          <Button
            variant="primary"
            size="md"
            disabled={!canAdd || busy}
            onClick={addRule}
          >
            {t("hikaya.learning.rules.add")}
          </Button>
        </div>
        {rules.length === 0 ? (
          <p className="px-4 py-3 text-sm text-ink-3">
            {t("hikaya.learning.rules.empty")}
          </p>
        ) : (
          rules.map((rule) => (
            <div
              key={rule.id}
              className="flex items-center justify-between gap-3 px-4 py-2"
            >
              <div className="flex items-center gap-2 min-w-0">
                <Replacement from={rule.pattern} to={rule.replacement} />
                {rule.source === "learned" && (
                  <Badge variant="secondary">
                    {t("hikaya.learning.rules.learned")}
                  </Badge>
                )}
              </div>
              <div className="flex items-center gap-3 shrink-0">
                <Switch
                  checked={rule.enabled}
                  disabled={busy}
                  ariaLabel={t("hikaya.learning.rules.enable", {
                    pattern: rule.pattern,
                  })}
                  onChange={(enabled) =>
                    change(
                      () => commands.hikayaSetRuleEnabled(rule.id, enabled),
                      "hikaya.learning.errors.save",
                    )
                  }
                />
                <Button
                  variant="danger-ghost"
                  size="sm"
                  disabled={busy}
                  aria-label={t("hikaya.learning.rules.delete", {
                    pattern: rule.pattern,
                  })}
                  title={t("hikaya.learning.rules.delete", {
                    pattern: rule.pattern,
                  })}
                  onClick={() =>
                    change(
                      () => commands.hikayaDeleteRule(rule.id),
                      "hikaya.learning.errors.delete",
                    )
                  }
                >
                  <Trash2 width={14} height={14} />
                </Button>
              </div>
            </div>
          ))
        )}
      </SettingsGroup>
    </div>
  );
};
