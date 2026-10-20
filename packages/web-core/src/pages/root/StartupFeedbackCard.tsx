import { useState } from 'react';
import { useTranslation } from 'react-i18next';

type FeedbackKind = 'works' | 'blocked';
const MAX_FEEDBACK_ENTRIES = 50;

export function StartupFeedbackCard() {
  const { t } = useTranslation('common');
  const [kind, setKind] = useState<FeedbackKind>('blocked');
  const [comment, setComment] = useState('');
  const [saved, setSaved] = useState(false);
  const [saveFailed, setSaveFailed] = useState(false);

  const saveFeedback = () => {
    try {
      const entry = {
        kind,
        comment: comment.trim(),
        page: window.location.pathname,
        createdAt: new Date().toISOString(),
      };
      const key = 'freeclaude-voice-feedback';
      const existing = JSON.parse(localStorage.getItem(key) ?? '[]');
      const feedback = Array.isArray(existing) ? existing : [];
      localStorage.setItem(
        key,
        JSON.stringify([...feedback, entry].slice(-MAX_FEEDBACK_ENTRIES))
      );
      setSaved(true);
      setSaveFailed(false);
    } catch {
      setSaved(false);
      setSaveFailed(true);
    }
  };

  return (
    <section className="mt-8 w-full max-w-md rounded-xl border border-border bg-secondary/60 p-4 text-left">
      <h2 className="text-sm font-semibold text-high">
        {t('landing.feedbackTitle')}
      </h2>
      <p className="mt-1 text-xs text-low">
        {t('landing.feedbackDescription')}
      </p>
      <div className="mt-3 flex gap-2">
        {(['works', 'blocked'] as const).map((option) => (
          <button
            key={option}
            type="button"
            aria-pressed={kind === option}
            onClick={() => {
              setKind(option);
              setSaved(false);
              setSaveFailed(false);
            }}
            className={`rounded-md border px-3 py-2 text-xs transition-colors ${kind === option ? 'border-brand bg-brand/10 text-high' : 'border-border text-low hover:text-high'}`}
          >
            {t(`landing.feedback.${option}`)}
          </button>
        ))}
      </div>
      <label className="mt-3 block text-xs text-low" htmlFor="startup-feedback">
        {t('landing.feedbackPrompt')}
      </label>
      <textarea
        id="startup-feedback"
        value={comment}
        onChange={(event) => {
          setComment(event.target.value);
          setSaved(false);
          setSaveFailed(false);
        }}
        rows={3}
        maxLength={2000}
        placeholder={t('landing.feedbackPlaceholder')}
        className="mt-1 w-full resize-y rounded-md border border-border bg-primary px-3 py-2 text-sm text-high outline-none focus:border-brand"
      />
      <div className="mt-2 flex items-center justify-between gap-3">
        <span aria-live="polite" className="text-xs text-low">
          {saveFailed
            ? t('landing.feedbackSaveError')
            : saved
              ? t('landing.feedbackSaved')
              : t('landing.feedbackLocalOnly')}
        </span>
        <button
          type="button"
          onClick={saveFeedback}
          className="rounded-md bg-brand px-3 py-2 text-xs font-medium text-white hover:opacity-90"
        >
          {t('landing.feedbackSave')}
        </button>
      </div>
    </section>
  );
}
