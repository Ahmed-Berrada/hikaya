import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "@/bindings";
import { notify } from "@/stores/noticeStore";
import { Button } from "../ui/Button";
import { Textarea } from "../ui/Textarea";

interface CorrectionEditorProps {
  entryId: number;
  text: string;
  onClose: () => void;
}

/**
 * Lets the user fix a dictation in History. Saving replaces the entry's text
 * and records the changed words, which is what Hikaya learns from.
 */
export const CorrectionEditor: React.FC<CorrectionEditorProps> = ({
  entryId,
  text,
  onClose,
}) => {
  const { t } = useTranslation();
  const [draft, setDraft] = useState(text);
  const [saving, setSaving] = useState(false);
  // Set once the correction is saved: what to tell the user before closing.
  const [confirmation, setConfirmation] = useState<string | null>(null);

  useEffect(() => {
    if (confirmation === null) return;
    const timer = setTimeout(onClose, 4000);
    return () => clearTimeout(timer);
  }, [confirmation, onClose]);
  const unchanged = draft.trim() === text.trim() || draft.trim().length === 0;

  const save = async () => {
    setSaving(true);
    const result = await commands.hikayaCorrectHistoryEntry(entryId, draft);
    setSaving(false);
    if (result.status === "error") {
      notify({ title: t("hikaya.correction.failed"), detail: result.error });
      return;
    }
    const learned = result.data;
    setConfirmation(
      learned.length > 0
        ? t("hikaya.correction.learned", {
            count: learned.length,
            list: learned.map((s) => `${s.from} → ${s.to}`).join(", "),
          })
        : t("hikaya.correction.savedNothingLearned"),
    );
  };

  if (confirmation !== null) {
    return (
      <p role="status" className="w-full pb-2 text-xs text-ink-3">
        {confirmation}
      </p>
    );
  }

  return (
    <div className="flex flex-col gap-2 w-full pb-2">
      <label className="flex flex-col gap-1 text-xs text-ink-3">
        {t("hikaya.correction.label")}
        <Textarea
          className="w-full font-normal"
          rows={Math.min(8, Math.max(2, Math.ceil(text.length / 80)))}
          value={draft}
          autoFocus
          disabled={saving}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") onClose();
          }}
        />
      </label>
      <p className="text-xs text-ink-3">{t("hikaya.correction.hint")}</p>
      <div className="flex justify-end gap-2">
        <Button
          variant="secondary"
          size="sm"
          disabled={saving}
          onClick={onClose}
        >
          {t("hikaya.correction.cancel")}
        </Button>
        <Button
          variant="primary"
          size="sm"
          disabled={unchanged || saving}
          onClick={save}
        >
          {t("hikaya.correction.save")}
        </Button>
      </div>
    </div>
  );
};
