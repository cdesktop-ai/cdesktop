import { forwardRef, useCallback, useImperativeHandle, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { AskUserQuestionItem, QuestionAnswer } from 'shared/types';
import { QuestionIcon } from '@phosphor-icons/react';

export interface AskUserQuestionBannerHandle {
  /** Fill the first unanswered question from the editor, submitting once the batch is complete. */
  submitCustomAnswer: (text: string) => void;
}

interface AskUserQuestionBannerProps {
  questions: AskUserQuestionItem[];
  onSubmitAnswers: (answers: QuestionAnswer[]) => void;
  isSubmitting: boolean;
  isTimedOut: boolean;
  error: string | null;
}

export const AskUserQuestionBanner = forwardRef<
  AskUserQuestionBannerHandle,
  AskUserQuestionBannerProps
>(function AskUserQuestionBanner(
  { questions, onSubmitAnswers, isSubmitting, isTimedOut, error },
  ref
) {
  const { t } = useTranslation('common');
  const [answers, setAnswers] = useState<Record<number, string[]>>({});
  const [customAnswers, setCustomAnswers] = useState<Record<number, string>>(
    {}
  );
  const disabled = isSubmitting || isTimedOut;

  const toQuestionAnswers = useCallback(
    (
      selected: Record<number, string[]>,
      custom: Record<number, string>
    ): QuestionAnswer[] =>
      questions.map((question, index) => {
        const labels = selected[index] ?? [];
        const text = custom[index]?.trim();
        return {
          question: question.question,
          answer: text
            ? question.multiSelect
              ? labels.includes(text)
                ? labels
                : [...labels, text]
              : [text]
            : labels,
        };
      }),
    [questions]
  );

  const questionAnswers = toQuestionAnswers(answers, customAnswers);
  const answeredCount = questionAnswers.filter(
    (answer) => answer.answer.length > 0
  ).length;
  const isAllAnswered =
    questions.length > 0 && answeredCount === questions.length;

  const handleSelectOption = useCallback(
    (question: AskUserQuestionItem, index: number, label: string) => {
      if (disabled) return;
      setAnswers((current) => {
        if (!question.multiSelect) {
          return { ...current, [index]: [label] };
        }
        const selected = new Set(current[index] ?? []);
        if (selected.has(label)) selected.delete(label);
        else selected.add(label);
        return { ...current, [index]: [...selected] };
      });
      if (!question.multiSelect) {
        setCustomAnswers((current) => ({
          ...current,
          [index]: '',
        }));
      }
    },
    [disabled]
  );

  const handleSubmit = useCallback(() => {
    if (disabled || !isAllAnswered) return;
    onSubmitAnswers(questionAnswers);
  }, [disabled, isAllAnswered, onSubmitAnswers, questionAnswers]);

  const handleCustomAnswer = useCallback(
    (question: AskUserQuestionItem, index: number, value: string) => {
      setCustomAnswers((current) => ({
        ...current,
        [index]: value,
      }));
      if (!question.multiSelect && value.trim()) {
        setAnswers((current) => ({ ...current, [index]: [] }));
      }
    },
    []
  );

  useImperativeHandle(
    ref,
    () => ({
      submitCustomAnswer: (text: string) => {
        if (disabled || !text.trim()) return;
        const targetIndex = questionAnswers.findIndex(
          (answer) => answer.answer.length === 0
        );
        if (targetIndex < 0) return;
        const nextCustom = { ...customAnswers, [targetIndex]: text.trim() };
        setCustomAnswers(nextCustom);
        const nextAnswers = toQuestionAnswers(answers, nextCustom);
        if (nextAnswers.every((answer) => answer.answer.length > 0)) {
          onSubmitAnswers(nextAnswers);
        }
      },
    }),
    [
      disabled,
      answers,
      customAnswers,
      questionAnswers,
      onSubmitAnswers,
      toQuestionAnswers,
    ]
  );

  return (
    <div className="border-b">
      {/* Header */}
      <div className="flex items-center gap-base px-double py-base">
        <QuestionIcon className="h-4 w-4 text-brand flex-shrink-0" />
        <span className="text-sm text-normal flex-1">
          {t('askQuestion.title')}
          {questions.length > 1 && (
            <span className="text-low ml-1">
              ({answeredCount}/{questions.length})
            </span>
          )}
        </span>
      </div>

      <div className="px-double pb-base space-y-base max-h-80 overflow-y-auto">
        {questions.map((question, index) => {
          const selected = new Set(answers[index] ?? []);
          return (
            <div
              key={`${question.question}-${index}`}
              className="rounded-md border border-border p-base"
            >
              <div className="flex items-center gap-base mb-base">
                <span className="text-xs font-medium text-low bg-secondary px-1 py-0.5 rounded">
                  {question.header}
                </span>
                {question.multiSelect && (
                  <span className="text-xs text-low">
                    {t('askQuestion.selectMultiple')}
                  </span>
                )}
              </div>
              <p className="text-sm font-medium text-normal mb-base">
                {question.question}
              </p>
              <div className="flex flex-wrap gap-base">
                {question.options.map((option) => {
                  const isSelected = selected.has(option.label);
                  return (
                    <button
                      key={option.label}
                      type="button"
                      disabled={disabled}
                      onClick={() =>
                        handleSelectOption(question, index, option.label)
                      }
                      className={`
                        group relative rounded-md border px-2.5 py-1.5 text-xs transition-all
                        ${
                          isSelected
                            ? 'border-brand bg-brand/10 text-normal'
                            : 'border-border text-low hover:border-brand/40 hover:text-normal hover:bg-accent'
                        }
                        ${disabled ? 'opacity-50 cursor-not-allowed' : 'cursor-pointer'}
                      `}
                      title={option.description}
                    >
                      <span className="font-medium">{option.label}</span>
                    </button>
                  );
                })}
              </div>
              <input
                type="text"
                disabled={disabled}
                value={customAnswers[index] ?? ''}
                onChange={(event) =>
                  handleCustomAnswer(question, index, event.target.value)
                }
                placeholder="Or type a custom answer"
                aria-label={`Custom answer for ${question.question}`}
                className="mt-base w-full rounded-md border border-border bg-primary px-2.5 py-1.5 text-xs text-normal placeholder:text-low focus:border-brand focus:outline-none disabled:opacity-50"
              />
            </div>
          );
        })}
        <div className="flex justify-end">
          <button
            type="button"
            disabled={disabled || !isAllAnswered}
            onClick={handleSubmit}
            className="rounded-md bg-brand px-3 py-1.5 text-xs font-medium text-white hover:bg-brand/90 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {t('askQuestion.confirmSelection')}
          </button>
        </div>
      </div>

      {error && (
        <div className="px-double pb-base text-sm text-error">{error}</div>
      )}

      {isSubmitting && (
        <div className="px-double pb-base text-sm text-low">
          {t('askQuestion.submitting')}
        </div>
      )}
    </div>
  );
});
